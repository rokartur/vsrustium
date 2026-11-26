//! localHistoryCommands.ts and localHistoryTimeline.ts: the Command Palette commands (Find Entry to Restore, Delete
//! All, Create Entry), the Timeline's Local History source and its entry commands.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use crate::commands::Outcome;
use crate::dialog;
use crate::editor::{TextDocument, uri_string};
use crate::editor_history;
use crate::local_history::{self, Entry as HistoryEntry};
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput, path_label};
use crate::timeline::{LOCAL_HISTORY_SOURCE, TimelineProvider, TimelineResult};

/// `LocalHistoryFileSystemProvider.SCHEMA`: a history entry opened as a read-only editor.
pub const LOCAL_HISTORY_SCHEME: &str = "vscode-local-history";
/// workingCopyHistoryTracker.ts `localHistoryRestore.source`.
const RESTORE_SOURCE: &str = "localHistoryRestore.source";

impl Workbench {
    pub(super) fn register_local_history_commands(&mut self) {
        self.commands.register("workbench.action.localHistory.restoreViaPicker", |workbench, _, window, cx| {
            workbench.show_local_history_files(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.deleteAll", |workbench, _, window, cx| {
            workbench.delete_all_local_history(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.create", |workbench, _, window, cx| {
            workbench.show_create_local_history_entry(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.compareWithFile", |workbench, args, window, cx| {
            let Some((file, entry, _)) = workbench.find_local_history_entry(args.first()) else { return Outcome::done() };
            let args = workbench.diff_with_file_arguments(&file, &entry, Value::Null);
            workbench.execute_command("_workbench.diff", args, window, cx)
        });
        self.commands.register("workbench.action.localHistory.compareWithPrevious", |workbench, args, window, cx| {
            let Some((file, entry, previous)) = workbench.find_local_history_entry(args.first()) else { return Outcome::done() };
            let Some(previous) = previous else {
                workbench.open_local_history_entry(&file, &entry, false, window, cx);
                return Outcome::done();
            };
            let args = workbench.diff_entries_arguments(&file, &previous, &entry);
            workbench.execute_command("_workbench.diff", args, window, cx)
        });
        self.commands.register("workbench.action.localHistory.selectForCompare", |workbench, args, _, _| {
            if workbench.find_local_history_entry(args.first()).is_some() {
                workbench.local_history_selected_for_compare = args.first().cloned();
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.compareWithSelected", |workbench, args, window, cx| {
            let selected = workbench.local_history_selected_for_compare.clone();
            let Some((_, selected, _)) = workbench.find_local_history_entry(selected.as_ref()) else { return Outcome::done() };
            let Some((file, entry, _)) = workbench.find_local_history_entry(args.first()) else { return Outcome::done() };
            let args = workbench.diff_entries_arguments(&file, &selected, &entry);
            workbench.execute_command("_workbench.diff", args, window, cx)
        });
        self.commands.register("workbench.action.localHistory.open", |workbench, args, window, cx| {
            if let Some((file, entry, _)) = workbench.find_local_history_entry(args.first()) {
                workbench.open_local_history_entry(&file, &entry, false, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.restore", |workbench, args, window, cx| {
            workbench.restore_local_history_entry(args.first(), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.restoreViaEditor", |workbench, args, window, cx| {
            // The editor title passes the entry's resource, else the active editor's.
            let path = match args.first().map(uri_parts) {
                Some(Ok((LOCAL_HISTORY_SCHEME, path))) => Some(path),
                _ => workbench.editors.active().filter(|editor| editor.document().scheme == LOCAL_HISTORY_SCHEME).map(|editor| editor.document().path.clone()),
            };
            let Some((location, file)) = path.as_deref().and_then(entry_parts) else { return Outcome::done() };
            let handle = location.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let item = json!({ "handle": handle, "uri": crate::editor::uri_value("file", &file) });
            workbench.restore_local_history_entry(Some(&item), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.rename", |workbench, args, window, cx| {
            workbench.show_rename_local_history_entry(args.first(), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.localHistory.delete", |workbench, args, window, cx| {
            workbench.delete_local_history_entries(&args, window, cx);
            Outcome::done()
        });
        self.update_local_history_timeline_registration();
    }

    /// localHistoryTimeline.ts `updateTimelineRegistration`: the Local History source exists while
    /// `workbench.localHistory.enabled` is on.
    pub(super) fn update_local_history_timeline_registration(&mut self) {
        let registered = self.timeline.model.sources().iter().any(|source| source.id == LOCAL_HISTORY_SOURCE);
        let enabled = self.setting("workbench.localHistory.enabled") == json!(true);
        if enabled && !registered {
            self.register_exclude_source_command(LOCAL_HISTORY_SOURCE);
            self.timeline.model.register(TimelineProvider { id: LOCAL_HISTORY_SOURCE.into(), label: "Local History".into(), schemes: vec!["*".into()] });
        } else if !enabled && registered && !self.timeline.model.unregister(LOCAL_HISTORY_SOURCE) {
            self.commands.unregister(&format!("timeline.toggleExcludeSource:{LOCAL_HISTORY_SOURCE}"));
        }
    }

    /// `onDidChangeWorkingCopyHistoryEntry`: the Local History source of every resource changed.
    pub(super) fn local_history_changed(&mut self, cx: &mut Context<Self>) {
        self.timeline_changed(LOCAL_HISTORY_SOURCE, false, None, cx);
    }

    /// localHistoryTimeline.ts `provideTimeline`: the entries of the file `uri` names, or of the file a local history
    /// entry belongs to.
    pub(super) fn local_history_timeline(&self, uri: &Value) -> TimelineResult {
        let file = match uri_parts(uri) {
            Ok(("file", path)) => Some(path),
            Ok((LOCAL_HISTORY_SCHEME, path)) => entry_parts(&path).map(|(_, file)| file),
            // shortcut: resources of extension file system providers show no entries; upstream maps them to `file`.
            _ => None,
        };
        let items: Vec<Value> = file.map(|file| self.local_history_entries(&file).iter().map(|entry| self.local_history_timeline_item(&file, entry)).collect()).unwrap_or_default();
        TimelineResult::from_value(&json!({ "source": LOCAL_HISTORY_SOURCE, "items": items })).expect("items is an array")
    }

    /// `toTimelineItem`.
    fn local_history_timeline_item(&self, file: &Path, entry: &HistoryEntry) -> Value {
        let label = local_history::source_label(entry.source.as_deref());
        let description = entry.source_description.as_ref().map(|description| format!(" ({description})")).unwrap_or_default();
        json!({
            "handle": entry.id,
            "label": label,
            "tooltip": { "value": format!("$(history) {}\n\n{label}{description}", local_history::date_label(entry.timestamp)), "supportThemeIcons": true },
            "source": LOCAL_HISTORY_SOURCE,
            "timestamp": entry.timestamp,
            "themeIcon": { "id": "circle-outline" },
            "contextValue": "localHistory:item",
            "resourceUri": self.local_history_entry_uri(file, entry),
            "command": { "id": "_workbench.diff", "title": "Compare with File", "arguments": self.diff_with_file_arguments(file, entry, Value::Null) },
        })
    }

    /// `getEntries`: the file's entries, oldest first.
    fn local_history_entries(&self, file: &Path) -> Vec<HistoryEntry> {
        let folder = local_history::entries_folder(&self.history_home(), &uri_string("file", file));
        local_history::read_entries(&folder, file).unwrap_or_else(|error| {
            eprintln!("local history of {}: {error}", file.display());
            Vec::new()
        })
    }

    /// `findLocalHistoryEntry`: the file, the entry the timeline item's `handle` names and the one before it.
    fn find_local_history_entry(&self, item: Option<&Value>) -> Option<(PathBuf, HistoryEntry, Option<HistoryEntry>)> {
        let item = item?;
        let file = match uri_parts(&item["uri"]).ok()? {
            (LOCAL_HISTORY_SCHEME, path) => entry_parts(&path)?.1,
            (_, path) => path,
        };
        let entries = self.local_history_entries(&file);
        let index = entries.iter().position(|entry| item["handle"].as_str() == Some(&entry.id))?;
        let previous = index.checked_sub(1).map(|previous| entries[previous].clone());
        Some((file, entries[index].clone(), previous))
    }

    /// `LocalHistoryFileSystemProvider.toLocalHistoryFileSystem`: the file's URI with the local history scheme and
    /// the entry's location and file in the query.
    fn local_history_entry_uri(&self, file: &Path, entry: &HistoryEntry) -> Value {
        let location = local_history::entries_folder(&self.history_home(), &uri_string("file", file)).join(&entry.id);
        // Written by hand: JSON.stringify keeps `location` first, serde_json's map sorts its keys.
        let query = format!("{{\"location\":{},\"associatedResource\":{}}}", json!(uri_string("file", &location)), json!(uri_string("file", file)));
        json!({ "$mid": 1, "scheme": LOCAL_HISTORY_SCHEME, "path": file.to_string_lossy(), "query": query })
    }

    /// `toDiffEditorArguments(entry, resource)`.
    fn diff_with_file_arguments(&self, file: &Path, entry: &HistoryEntry, options: Value) -> Vec<Value> {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let label = format!("{name} ({} \u{2022} {}) \u{2194} {name}", local_history::source_label(entry.source.as_deref()), local_history::date_label(entry.timestamp));
        let options = if options.is_null() { Value::Null } else { json!([null, options]) };
        vec![self.local_history_entry_uri(file, entry), crate::editor::uri_value("file", file), json!(label), options]
    }

    /// `toDiffEditorArguments(previousEntry, entry)`.
    fn diff_entries_arguments(&self, file: &Path, previous: &HistoryEntry, entry: &HistoryEntry) -> Vec<Value> {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let describe = |entry: &HistoryEntry| format!("{name} ({} \u{2022} {})", local_history::source_label(entry.source.as_deref()), local_history::date_label(entry.timestamp));
        let label = format!("{} \u{2194} {}", describe(previous), describe(entry));
        vec![self.local_history_entry_uri(file, previous), self.local_history_entry_uri(file, entry), json!(label), Value::Null]
    }

    /// `LocalHistoryFileSystemProvider.readFile`: a model of the entry at `path` (the file's path and the query), read
    /// from its location.
    pub(super) fn open_local_history_model(&mut self, path: &Path) -> Result<(), String> {
        let document = self.local_history_document(path)?;
        self.editors.open_model(document);
        Ok(())
    }

    fn local_history_document(&self, path: &Path) -> Result<TextDocument, String> {
        let (location, _) = entry_parts(path).ok_or_else(|| format!("Unable to resolve resource {}", path.display()))?;
        let bytes = std::fs::read(&location).map_err(|error| format!("Unable to read file '{}' ({error})", location.display()))?;
        Ok(TextDocument::from_bytes(LOCAL_HISTORY_SCHEME, path, &bytes, None, &self.languages, &self.settings.lock().unwrap()))
    }

    /// localHistoryCommands.ts `restore`: after the confirmation the entry's contents replace the file and its models,
    /// the file opens, a "File Restored" entry is added and the entry's editors close.
    fn restore_local_history_entry(&mut self, item: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((file, entry, _)) = self.find_local_history_entry(item) else { return };
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let message = format!("Do you want to restore the contents of '{name}'?");
        let answer = dialog::show_message_box(window, PromptLevel::Warning, &message, Some("Restoring will discard any unsaved changes."), &["Restore", "Cancel"], None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok((0, _)) = answer.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let location = local_history::entries_folder(&workbench.history_home(), &uri_string("file", &file)).join(&entry.id);
                if let Err(error) = std::fs::copy(&location, &file) {
                    drop(dialog::show_message_box(window, PromptLevel::Critical, &format!("Unable to restore '{name}'."), Some(&error.to_string()), &["OK"], None, cx));
                    return;
                }
                if let Ok(bytes) = std::fs::read(&file) {
                    workbench.revert_model("file", &file, &bytes);
                }
                workbench.open_file(file.clone(), false, window, cx);
                let settings = workbench.settings.lock().unwrap();
                let limits = local_history::limits(&settings, &file);
                drop(settings);
                if let Some(limits) = limits {
                    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
                    if let Err(error) = local_history::add_entry(&workbench.history_home(), &uri_string("file", &file), &file, Some(RESTORE_SOURCE), None, timestamp, &limits) {
                        eprintln!("local history of {}: {error}", file.display());
                    }
                }
                workbench.close_local_history_entry(&file, &entry, window, cx);
                workbench.local_history_changed(cx);
            });
        })
        .detach();
    }

    /// `closeEntry`: the editors of the entry.
    fn close_local_history_entry(&mut self, file: &Path, entry: &HistoryEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Ok((_, path)) = uri_parts(&self.local_history_entry_uri(file, entry)) else { return };
        let indices: Vec<usize> = self.editors.editors.iter().enumerate().filter(|(_, editor)| editor.document().is(LOCAL_HISTORY_SCHEME, &path)).map(|(index, _)| index).collect();
        if !indices.is_empty() {
            drop(self.close_editors(indices, window, cx));
        }
    }

    /// `Rename` outside an inline row: an input box with the entry's name.
    fn show_rename_local_history_entry(&mut self, item: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((file, entry, _)) = self.find_local_history_entry(item) else { return };
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.title = Some("Rename Local History Entry".into());
        input.ignore_focus_out = true;
        input.placeholder = Some("Enter the new name of the local history entry".into());
        input.input_box.set_value(local_history::source_label(entry.source.as_deref()).to_string());
        let id = self.quick_inputs.add(input, Owner::LocalHistoryRename(file, entry.id));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn rename_local_history_entry_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::LocalHistoryRename(file, entry_id)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::Accept(_) => {
                let (file, entry_id) = (file.clone(), entry_id.clone());
                let name = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()).unwrap_or_default();
                self.hide_quick_input_by_id(id, window, cx);
                if name.is_empty() {
                    return;
                }
                if let Err(error) = local_history::rename_entry(&self.history_home(), &uri_string("file", &file), &file, &entry_id, &name) {
                    eprintln!("local history of {}: {error}", file.display());
                }
                self.local_history_changed(cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `Delete`: the item, or the selected items, after the confirmation.
    fn delete_local_history_entries(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<Value> = match args.get(3) {
            Some(Value::Array(selection)) => selection.clone(),
            _ => args.first().cloned().into_iter().collect(),
        };
        let entries: Vec<(PathBuf, HistoryEntry)> = items.iter().filter_map(|item| self.find_local_history_entry(Some(item))).map(|(file, entry, _)| (file, entry)).collect();
        let Some((first_file, first_entry)) = entries.first() else { return };
        let message = match entries.len() {
            1 => format!("Do you want to delete the local history entry of '{}' from {}?", first_file.file_name().unwrap_or_default().to_string_lossy(), local_history::date_label(first_entry.timestamp)),
            count => format!("Do you want to delete {count} local history entries?"),
        };
        let answer = dialog::show_message_box(window, PromptLevel::Warning, &message, Some("This action is irreversible!"), &["Delete", "Cancel"], None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok((0, _)) = answer.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                for (file, entry) in &entries {
                    if let Err(error) = local_history::remove_entry(&workbench.history_home(), &uri_string("file", file), file, &entry.id) {
                        eprintln!("local history of {}: {error}", file.display());
                    }
                    workbench.close_local_history_entry(file, entry, window, cx);
                }
                workbench.local_history_changed(cx);
            });
        })
        .detach();
    }

    fn history_home(&self) -> PathBuf {
        self.settings.lock().unwrap().user_home().join("History")
    }

    /// `restoreViaPicker` first step: the files with history, those of the editor history first, the rest by path.
    fn show_local_history_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut rest = local_history::all_files(&self.history_home());
        let mut files = Vec::new();
        for recent in &self.quick_access.editor_history.entries {
            if let Some(index) = rest.iter().position(|file| file == recent) {
                files.push(rest.remove(index));
            }
        }
        rest.sort();
        files.extend(rest);
        let workspace = self.workspace();
        let items = files.iter().enumerate().map(|(handle, file)| {
            Entry::Item(Item {
                handle: handle as i64,
                label: file.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                description: file.parent().map(|parent| path_label(parent, workspace)),
                pickable: true,
                file_icon: Some(file.clone()),
                ..Item::default()
            })
        });
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select the file to show local history for".into());
        input.match_on_label = true;
        input.match_on_description = true;
        input.set_items(items.collect());
        let id = self.quick_inputs.add(input, Owner::LocalHistoryFiles(files));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn local_history_files_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::LocalHistoryFiles(files)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                let file = files[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                self.show_local_history_entries(file, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `restoreViaPicker` second step: the file's entries, newest first.
    fn show_local_history_entries(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let folder = local_history::entries_folder(&self.history_home(), &uri_string("file", &file));
        let mut entries = local_history::read_entries(&folder, &file).unwrap_or_else(|error| {
            eprintln!("local history of {}: {error}", file.display());
            Vec::new()
        });
        entries.reverse();
        let items = entries.iter().enumerate().map(|(handle, entry)| {
            Entry::Item(Item {
                handle: handle as i64,
                label: format!("$(circle-outline) {}", local_history::source_label(entry.source.as_deref())),
                description: Some(local_history::date_label(entry.timestamp)),
                pickable: true,
                ..Item::default()
            })
        });
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select the local history entry to open".into());
        input.can_accept_in_background = true;
        input.match_on_label = true;
        input.match_on_description = true;
        input.set_items(items.collect());
        let id = self.quick_inputs.add(input, Owner::LocalHistoryEntries(file, entries));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn local_history_entries_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::LocalHistoryEntries(file, entries)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::Accept(in_background) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                let (file, entry) = (file.clone(), entries[handle as usize].clone());
                if !in_background {
                    self.hide_quick_input_by_id(id, window, cx);
                }
                if file.exists() {
                    let args = self.diff_with_file_arguments(&file, &entry, json!({ "preserveFocus": in_background }));
                    drop(self.execute_command("_workbench.diff", args, window, cx));
                    return;
                }
                self.open_local_history_entry(&file, &entry, in_background, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `openEntry`: the entry's contents in a read-only editor titled with the file name, source and date.
    fn open_local_history_entry(&mut self, file: &Path, entry: &HistoryEntry, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Ok((_, path)) = uri_parts(&self.local_history_entry_uri(file, entry)) else { return };
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let label = format!("{name} ({} \u{2022} {})", local_history::source_label(entry.source.as_deref()), local_history::date_label(entry.timestamp));
        match self.editors.editors.iter().position(|editor| editor.document().is(LOCAL_HISTORY_SCHEME, &path)) {
            Some(index) => self.editors.activate(index),
            None => {
                let document = match self.local_history_document(&path) {
                    Ok(document) => document,
                    Err(error) => return self.notify_error(error),
                };
                let enable_preview = self.setting("workbench.editor.enablePreview").as_bool().unwrap_or(true);
                self.editors.open(document, enable_preview && !in_background);
            }
        }
        self.local_history_labels.insert(path, label);
        match in_background {
            true => window.focus(&self.quick_inputs.focus, cx),
            false => window.focus(&self.editor_focus, cx),
        }
        self.editors_changed(cx);
    }

    /// `Delete All`: after the confirmation, every entry of every file.
    fn delete_all_local_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = "Do you want to delete all entries of all files in local history?";
        let answer = dialog::show_message_box(window, PromptLevel::Warning, message, Some("This action is irreversible!"), &["Delete All", "Cancel"], None, cx);
        let history_home = self.history_home();
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok((0, _)) = answer.await else { return };
            let result = local_history::remove_all(&history_home);
            let _ = workbench.update(cx, |workbench, cx| {
                if let Err(error) = result {
                    workbench.notify_error(format!("Unable to delete the local history ({error})"));
                }
                workbench.local_history_changed(cx);
            });
        })
        .detach();
    }

    /// `Create Entry`: an input box for the new entry's source, for the active editor's file.
    fn show_create_local_history_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.editors.active().filter(|editor| editor.document().scheme == "file").map(|editor| editor.document().path.clone()) else { return };
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.title = Some("Create Local History Entry".into());
        input.ignore_focus_out = true;
        input.placeholder = Some(format!("Enter the new name of the local history entry for '{}'", file.file_name().unwrap_or_default().to_string_lossy()));
        let id = self.quick_inputs.add(input, Owner::LocalHistoryCreate(file));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn create_local_history_entry_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::LocalHistoryCreate(file)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::Accept(_) => {
                let file = file.clone();
                let source = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()).unwrap_or_default();
                self.hide_quick_input_by_id(id, window, cx);
                if source.is_empty() {
                    return;
                }
                let settings = self.settings.lock().unwrap();
                let Some(limits) = local_history::limits(&settings, &file) else { return };
                drop(settings);
                let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64);
                if let Err(error) = local_history::add_entry(&self.history_home(), &uri_string("file", &file), &file, Some(&source), None, timestamp, &limits) {
                    self.notify_error(format!("Unable to create a local history entry for '{}' ({error})", file.display()));
                }
                self.local_history_changed(cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

/// `fromLocalHistoryFileSystem`: the entry's location and its file, from the path of an entry's model, the file's
/// path followed by `?` and the query.
fn entry_parts(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let path = path.to_string_lossy();
    let (_, query) = path.split_once("?{")?;
    let query: Value = serde_json::from_str(&format!("{{{query}")).ok()?;
    Some((editor_history::file_path(query["location"].as_str()?)?, editor_history::file_path(query["associatedResource"].as_str()?)?))
}
