//! localHistoryCommands.ts commands of the Command Palette: Find Entry to Restore, Delete All and Create Entry.
//! The Timeline's entry commands (compare, restore, rename, delete) arrive with the Timeline view (milestone 14).

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use gpui::{Context, PromptLevel, Window};

use super::Workbench;
use crate::commands::Outcome;
use crate::dialog;
use crate::editor::{TextDocument, uri_string};
use crate::local_history::{self, Entry as HistoryEntry};
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput, path_label};

/// `LocalHistoryFileSystemProvider.SCHEMA`: a history entry opened as a read-only editor.
pub const LOCAL_HISTORY_SCHEME: &str = "vscode-local-history";

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
                // shortcut: upstream compares an entry of an existing file with it in the diff editor; the port opens the
                // entry's contents (`openEntry`) until the diff editor lands in milestone 14.
                self.open_local_history_entry(&file, &entry, in_background, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `openEntry`: the entry's contents in a read-only editor titled with the file name, source and date.
    fn open_local_history_entry(&mut self, file: &std::path::Path, entry: &HistoryEntry, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let location = local_history::entries_folder(&self.history_home(), &uri_string("file", file)).join(&entry.id);
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let label = format!("{name} ({} \u{2022} {})", local_history::source_label(entry.source.as_deref()), local_history::date_label(entry.timestamp));
        match self.editors.find(&location) {
            Some(index) => self.editors.activate(index),
            None => {
                let bytes = match std::fs::read(&location) {
                    Ok(bytes) => bytes,
                    Err(error) => return self.notify_error(format!("Unable to read file '{}' ({error})", location.display())),
                };
                let document = TextDocument::from_bytes(LOCAL_HISTORY_SCHEME, &location, &bytes, None, &self.languages, &self.settings.lock().unwrap());
                let enable_preview = self.setting("workbench.editor.enablePreview").as_bool().unwrap_or(true);
                self.editors.open(document, enable_preview && !in_background);
            }
        }
        self.local_history_labels.insert(location, label);
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
            if let Err(error) = local_history::remove_all(&history_home) {
                let _ = workbench.update(cx, |workbench, _| workbench.notify_error(format!("Unable to delete the local history ({error})")));
            }
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
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}
