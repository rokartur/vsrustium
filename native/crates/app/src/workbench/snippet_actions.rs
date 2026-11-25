//! The snippets contribution's commands: insertSnippet.ts (Insert Snippet), surroundWithSnippet.ts (Surround with
//! Snippet...), fileTemplateSnippets.ts (Fill File with Snippet), configureSnippets.ts (Configure Snippets), and
//! snippetPicker.ts `pickSnippet` with its Hide from IntelliSense buttons.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_status::label_item;
use super::snippet_completions::snippet_language;
use super::snippet_controller::SnippetInsert;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};
use crate::snippets::{Snippet, SnippetGetOptions, SnippetSource};

/// The handles the snippet code action providers answer to (snippetCodeActionProvider.ts).
pub(super) const SURROUND_WITH_SNIPPET_PROVIDER: u64 = u64::MAX - 3;
pub(super) const FILE_TEMPLATE_PROVIDER: u64 = u64::MAX - 4;
/// `_MAX_CODE_ACTIONS` of both providers.
const MAX_CODE_ACTIONS: usize = 4;
const SURROUND_WITH: &str = "refactor.surround";

/// What accepting a pick of `pickSnippet` does.
pub struct SnippetPicker {
    snippets: Vec<Snippet>,
}

/// The file name input of "New Global Snippets file..." and "New Snippets file for '...'".
pub struct SnippetFileInput {
    folder: PathBuf,
    scope: String,
}

/// A row of Configure Snippets: open an existing file, create a language file first, or name a new one.
enum ConfigurePick {
    Open(PathBuf),
    CreateLanguage(PathBuf, String),
    New(PathBuf, String),
}

impl Workbench {
    pub(super) fn register_snippet_actions(&mut self) {
        self.commands.register("editor.action.insertSnippet", |workbench, args, window, cx| {
            workbench.insert_snippet_action(args.first().unwrap_or(&Value::Null), window, cx);
            Outcome::done()
        });
        self.commands.register("editor.action.showSnippets", |workbench, args, window, cx| workbench.execute_command("editor.action.insertSnippet", args, window, cx));
        self.commands.register("editor.action.surroundWithSnippet", |workbench, _, window, cx| {
            let snippets = workbench.surroundable_snippets(true);
            if !snippets.is_empty() {
                workbench.pick_snippet(snippets, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.populateFileFromSnippet", |workbench, _, window, cx| {
            workbench.fill_file_with_snippet(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.openSnippets", |workbench, _, window, cx| {
            workbench.configure_snippets(window, cx);
            Outcome::done()
        });
    }

    /// `InsertSnippetAction.runEditorCommand`: `snippet` inserts the text, `name` the language's snippet of that name,
    /// else the picker of the language at the cursor (or `langId`).
    fn insert_snippet_action(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if let Some(body) = args["snippet"].as_str() {
            let snippet = Snippet { body: body.to_owned(), snippet_source: SnippetSource::User, snippet_identifier: String::new(), ..empty_snippet() };
            self.insert_picked_snippet(&snippet, cx);
            return;
        }
        let language = match args["langId"].as_str() {
            Some(language) if !self.languages.is_registered(language) => return,
            Some(language) => language.to_owned(),
            None => {
                let head = editor.selection().head;
                snippet_language(editor, head.line, head.column, &self.languages)
            }
        };
        let path = editor_path(editor);
        if let Some(name) = args["name"].as_str() {
            let options = SnippetGetOptions { include_no_prefix_snippets: true, ..SnippetGetOptions::default() };
            if let Some(snippet) = self.get_snippets(Some(&language), None, options).into_iter().find(|snippet| snippet.name == name) {
                self.insert_picked_snippet(&snippet, cx);
            }
            return;
        }
        let options = SnippetGetOptions { include_disabled_snippets: true, include_no_prefix_snippets: true, ..SnippetGetOptions::default() };
        let snippets = self.get_snippets(Some(&language), path.as_deref(), options);
        self.pick_snippet(snippets, window, cx);
    }

    /// `getSurroundableSnippets`: the snippets of the language at the cursor that use the selection.
    fn surroundable_snippets(&mut self, include_disabled_snippets: bool) -> Vec<Snippet> {
        let Some(editor) = self.editors.active() else { return Vec::new() };
        let head = editor.selection().head;
        let language = snippet_language(editor, head.line, head.column, &self.languages);
        let path = editor_path(editor);
        let options = SnippetGetOptions { include_no_prefix_snippets: true, include_disabled_snippets, ..SnippetGetOptions::default() };
        self.get_snippets(Some(&language), path.as_deref(), options).into_iter().filter(Snippet::uses_selection).collect()
    }

    /// `SnippetController2.insert` of the snippet at the selections, with the clipboard when it reads it, and
    /// `updateUsageTimestamp`.
    fn insert_picked_snippet(&mut self, snippet: &Snippet, cx: &mut Context<Self>) {
        let clipboard = snippet.needs_clipboard().then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten();
        let Some(editor) = self.editors.active() else { return };
        let snippets: Vec<_> = editor.selections.iter().map(|selection| (selection.start(), selection.end(), snippet.body.as_str(), false)).collect();
        self.insert_snippets(SnippetInsert { snippets: &snippets, clipboard: clipboard.as_deref(), undo_stop_before: true, undo_stop_after: true, reason: "snippet" }, cx);
        if !snippet.snippet_identifier.is_empty() {
            self.update_snippet_usage(&snippet.snippet_identifier);
        }
    }

    /// `pickSnippet`: the snippets by source with a separator per source, extension snippets with the eye button
    /// that hides or shows them in IntelliSense.
    fn pick_snippet(&mut self, mut snippets: Vec<Snippet>, window: &mut Window, cx: &mut Context<Self>) {
        snippets.sort_by_key(|snippet| snippet.snippet_source);
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a snippet".into());
        input.match_on_detail = true;
        input.ignore_focus_out = false;
        input.keep_scroll_position = true;
        let items = self.snippet_picks(&snippets);
        if items.is_empty() {
            input.validation_message = Some("No snippet available".into());
        }
        input.set_items(items);
        let id = self.quick_inputs.add(input, Owner::Snippets(SnippetPicker { snippets }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    fn snippet_picks(&self, snippets: &[Snippet]) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut previous: Option<&Snippet> = None;
        for (handle, snippet) in snippets.iter().enumerate() {
            if previous.is_none_or(|previous| previous.snippet_source != snippet.snippet_source || previous.source != snippet.source) {
                let label = match snippet.snippet_source {
                    SnippetSource::User => "User Snippets".to_owned(),
                    SnippetSource::Extension => snippet.source.clone(),
                    SnippetSource::Workspace => "Workspace Snippets".to_owned(),
                };
                entries.push(Entry::Separator(Some(label)));
            }
            let detail = if snippet.description.is_empty() { snippet.body.clone() } else { snippet.description.clone() };
            let label = if snippet.prefix.is_empty() { &snippet.name } else { &snippet.prefix };
            let mut item = Item { handle: handle as i64, detail: Some(detail), ..label_item(label) };
            if snippet.snippet_source == SnippetSource::Extension {
                let enabled = self.snippet_enabled(snippet);
                let (codicon, tooltip) = if enabled { ("eye-closed", "Hide from IntelliSense") } else { ("eye", "Show in IntelliSense") };
                if !enabled {
                    item.description = Some("(hidden from IntelliSense)".into());
                }
                item.buttons = vec![Button { handle: 0, codicon: Some(codicon.into()), tooltip: Some(tooltip.into()), ..Button::default() }];
            }
            entries.push(Entry::Item(item));
            previous = Some(snippet);
        }
        entries
    }

    pub(crate) fn snippet_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::Snippets(picker)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::TriggerItemButton(handle, _, _) => {
                let Some(snippet) = picker.snippets.get(handle as usize).cloned() else { return };
                let enabled = self.snippet_enabled(&snippet);
                self.set_snippet_enabled(&snippet, !enabled);
                let Some(Owner::Snippets(picker)) = self.quick_inputs.owner(id) else { return };
                let items = self.snippet_picks(&picker.snippets);
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.set_items(items);
                }
                self.flush_quick_input_events(window, cx);
            }
            Event::Accept(_) => {
                let picked = self.quick_inputs.get(id).and_then(|input| input.active_handles().first().copied());
                let snippet = picked.and_then(|handle| picker.snippets.get(handle as usize).cloned());
                self.hide_quick_input_by_id(id, window, cx);
                if let Some(snippet) = snippet {
                    window.focus(&self.editor_focus, cx);
                    self.insert_picked_snippet(&snippet, cx);
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `ApplyFileSnippetAction`: the file template snippets once per scope, grouped by language; the pick sets the
    /// language and replaces the whole file.
    fn fill_file_with_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let path = editor_path(editor);
        let options = SnippetGetOptions { file_template_snippets: Some(true), no_recency_sort: true, include_no_prefix_snippets: true, ..SnippetGetOptions::default() };
        let snippets = self.get_snippets(None, path.as_deref(), options);
        if snippets.is_empty() {
            return;
        }
        let mut all: Vec<(String, Snippet)> = Vec::new();
        for snippet in snippets {
            match snippet.scopes.is_empty() {
                true => all.push((String::new(), snippet)),
                false => all.extend(snippet.scopes.iter().map(|scope| (scope.clone(), snippet.clone()))),
            }
        }
        all.sort_by(|a, b| a.0.cmp(&b.0));
        let mut entries = Vec::new();
        for (handle, (language, snippet)) in all.iter().enumerate() {
            if handle == 0 || all[handle - 1].0 != *language {
                entries.push(Entry::Separator(Some(self.languages.name(language).to_owned())));
            }
            let label = if snippet.prefix.is_empty() { &snippet.name } else { &snippet.prefix };
            let detail = (!snippet.description.is_empty()).then(|| snippet.description.clone());
            entries.push(Entry::Item(Item { handle: handle as i64, detail, ..label_item(label) }));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a snippet".into());
        input.match_on_detail = true;
        input.set_items(entries);
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Some(handle) = picked.await.ok().and_then(Result::ok).and_then(|picked| picked.as_u64()) else { return };
            let (language, snippet) = all.swap_remove(handle as usize);
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let Some(editor) = workbench.editors.active() else { return };
                let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
                // The language first, so comment variables resolve against it.
                if workbench.languages.is_registered(&language) {
                    workbench.set_model_language(&scheme, &path, &language, cx);
                }
                let Some(editor) = workbench.editors.active() else { return };
                let end = crate::editor::Position::new(editor.document().lines.len() - 1, editor.document().lines.last().map_or(0, |line| line.chars().count()));
                let snippets = [(crate::editor::Position::new(0, 0), end, snippet.body.as_str(), false)];
                workbench.insert_snippets(SnippetInsert { snippets: &snippets, clipboard: None, undo_stop_before: true, undo_stop_after: true, reason: "snippet" }, cx);
                window.focus(&workbench.editor_focus, cx);
            });
        })
        .detach();
    }

    /// `ConfigureSnippetsAction`: the user and workspace snippet files, the new file entries, then a file for each
    /// language without one; the active editor's language first in its group, marked with `*`.
    fn configure_snippets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: std::collections::HashMap<String, String> = self.languages.sorted_names().into_iter().map(|(name, id)| (id.to_owned(), name.to_owned())).collect();
        let active_language = self.editors.active().map(|editor| editor.document().language_id.clone());
        let user_folder = self.settings.lock().unwrap().user_home().join("snippets");
        let folders: Vec<(String, PathBuf)> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| (folder.name.clone(), folder.path.clone())).collect()).unwrap_or_default();
        let workspace = self.workspace().cloned();
        let mut existing: Vec<(Item, ConfigurePick)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut global_details: std::collections::HashMap<String, (usize, String)> = std::collections::HashMap::new();
        for (location, _, file) in self.snippet_files() {
            if file.source == SnippetSource::Extension {
                continue;
            }
            let label = file_name(location);
            if file.is_global_snippets {
                let mut scope_names: Vec<String> = Vec::new();
                'outer: for snippet in &file.data {
                    for scope in &snippet.scopes {
                        let Some(name) = names.get(scope) else { continue };
                        if scope_names.contains(name) {
                            continue;
                        }
                        if scope_names.len() >= 4 {
                            scope_names.push(format!("{name}..."));
                            break 'outer;
                        }
                        scope_names.push(name.clone());
                    }
                }
                let description = if scope_names.is_empty() { "(global)".to_owned() } else { format!("({})", scope_names.join(", ")) };
                let mut item = Item { description: Some(description), ..label_item(&label) };
                // Files of the same name show their source and path.
                if let Some(source) = file.data.first().map(|snippet| snippet.source.clone()) {
                    let detail = format!("({source}) {}", crate::quick_input::path_label(location, workspace.as_ref()));
                    if let Some((index, other_detail)) = global_details.get(&label) {
                        item.detail = Some(detail.clone());
                        existing[*index].0.detail = Some(other_detail.clone());
                    }
                    global_details.insert(label.clone(), (existing.len(), detail));
                }
                existing.push((item, ConfigurePick::Open(location.clone())));
            } else {
                let language = label.trim_end_matches(".json").to_owned();
                let description = format!("({})", names.get(&language).cloned().unwrap_or_else(|| language.clone()));
                existing.push((Item { description: Some(description), ..label_item(&label) }, ConfigurePick::Open(location.clone())));
                seen.insert(language);
            }
        }
        existing.sort_by(|a, b| {
            let extension = |pick: &ConfigurePick| match pick {
                ConfigurePick::Open(path) => path.extension().map(|extension| extension.to_string_lossy().into_owned()),
                _ => None,
            };
            let (a_extension, b_extension) = (extension(&a.1), extension(&b.1));
            match (a_extension == b_extension, a_extension.as_deref() == Some("code-snippets")) {
                (true, _) => crate::command_palette::locale_compare(&a.0.label, &b.0.label),
                (false, true) => std::cmp::Ordering::Less,
                (false, false) => std::cmp::Ordering::Greater,
            }
        });
        let mut future: Vec<(Item, ConfigurePick)> = Vec::new();
        for (name, id) in self.languages.sorted_names() {
            if seen.contains(id) {
                continue;
            }
            let item = Item { description: Some(format!("({id})")), language_icon: Some(id.to_owned()), ..label_item(name) };
            future.push((item, ConfigurePick::CreateLanguage(user_folder.join(format!("{id}.json")), name.to_owned())));
        }
        future.sort_by(|a, b| crate::command_palette::locale_compare(&a.0.label, &b.0.label));
        if let Some(language) = &active_language {
            let existing_index = existing.iter().position(|(_, pick)| matches!(pick, ConfigurePick::Open(path) if file_name(path) == format!("{language}.json")));
            promote(&mut existing, existing_index);
            let future_index = future.iter().position(|(item, _)| item.label == *language);
            promote(&mut future, future_index);
        }
        let mut rows: Vec<(Option<Item>, Option<ConfigurePick>)> = Vec::new();
        let has_existing = !existing.is_empty();
        if has_existing {
            rows.push((None, Some(ConfigurePick::New(PathBuf::new(), "Existing Snippets".into()))));
        }
        rows.extend(existing.into_iter().map(|(item, pick)| (Some(item), Some(pick))));
        rows.push((None, Some(ConfigurePick::New(PathBuf::new(), "New Snippets".into()))));
        rows.push((Some(label_item("New Global Snippets file...")), Some(ConfigurePick::New(user_folder.clone(), "global".into()))));
        for (name, path) in folders {
            rows.push((Some(label_item(&format!("New Snippets file for '{name}'..."))), Some(ConfigurePick::New(path.join(".vscode"), format!("{name} workspace")))));
        }
        rows.extend(future.into_iter().map(|(item, pick)| (Some(item), Some(pick))));
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        for (item, pick) in rows {
            match (item, pick) {
                (None, Some(ConfigurePick::New(_, label))) => entries.push(Entry::Separator(Some(label))),
                (Some(item), Some(pick)) => {
                    entries.push(Entry::Item(Item { handle: picks.len() as i64, ..item }));
                    picks.push(pick);
                }
                _ => {}
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Snippets File or Create Snippets".into());
        input.match_on_description = true;
        input.set_items(entries);
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Some(handle) = picked.await.ok().and_then(Result::ok).and_then(|picked| picked.as_u64()) else { return };
            let pick = picks.swap_remove(handle as usize);
            let _ = workbench.update_in(cx, |workbench, window, cx| match pick {
                ConfigurePick::Open(path) => workbench.open_file(path, true, window, cx),
                ConfigurePick::CreateLanguage(path, label) => {
                    if !path.exists() {
                        let _ = std::fs::create_dir_all(path.parent().expect("in the snippets folder"));
                        if let Err(error) = std::fs::write(&path, language_snippets_template(&label)) {
                            return workbench.notify_error(format!("Unable to write '{}': {error}", path.display()));
                        }
                    }
                    workbench.open_file(path, true, window, cx);
                }
                ConfigurePick::New(folder, scope) => workbench.ask_snippet_file_name(folder, scope, window, cx),
            });
        })
        .detach();
    }

    /// `createSnippetFile`: the folder is created, then the name is asked and checked as typed.
    fn ask_snippet_file_name(&mut self, folder: PathBuf, scope: String, window: &mut Window, cx: &mut Context<Self>) {
        let _ = std::fs::create_dir_all(&folder);
        let mut input = QuickInput::new(0, Kind::InputBox);
        input.placeholder = Some("Type snippet file name".into());
        let id = self.quick_inputs.add(input, Owner::SnippetFileName(SnippetFileInput { folder, scope }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub(crate) fn snippet_file_name_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::SnippetFileName(file)) = self.quick_inputs.owner(id) else { return };
        let (folder, scope) = (file.folder.clone(), file.scope.clone());
        let value = self.quick_inputs.get(id).map(|input| input.input_box.value.clone()).unwrap_or_default();
        match event {
            Event::ChangeValue(value) => {
                let message = snippet_file_name_error(&folder, &value);
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.validation_message = message;
                }
                self.flush_quick_input_events(window, cx);
            }
            Event::Accept(_) if snippet_file_name_error(&folder, &value).is_none() => {
                self.hide_quick_input_by_id(id, window, cx);
                let path = snippet_file_path(&folder, &value);
                if let Err(error) = std::fs::write(&path, global_snippets_template(&scope)) {
                    return self.notify_error(format!("Unable to write '{}': {error}", path.display()));
                }
                self.open_file(path, true, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

impl Workbench {
    /// `SnippetCodeActions`: with `editor.snippets.codeActions.enabled`, the Surround With snippets of a non-empty
    /// range and the file templates of an empty model, each provider's list as `$provideCodeActions` answers it.
    pub(super) fn snippet_code_actions(&self, editor: &crate::editor::OpenEditor, range: &Value) -> Vec<(u64, Value)> {
        if !self.snippet_code_actions_enabled(editor) {
            return Vec::new();
        }
        let path = editor_path(editor);
        let edit = |range: &Value, snippet: &Snippet| {
            json!({ "edits": [{ "resource": editor.document().uri(), "versionId": editor.text_model().version_id, "textEdit": { "range": range, "text": snippet.body, "insertAsSnippet": true } }] })
        };
        let mut lists = Vec::new();
        let empty_range = range["startLineNumber"] == range["endLineNumber"] && range["startColumn"] == range["endColumn"];
        if !empty_range {
            // `getSurroundableSnippets` at the selection's position.
            let position = crate::editor::Position::new(range["endLineNumber"].as_u64().unwrap_or(1) as usize - 1, range["endColumn"].as_u64().unwrap_or(1) as usize - 1);
            let language = snippet_language(editor, position.line, position.column.min(editor.document().lines[position.line].chars().count()), &self.languages);
            let options = SnippetGetOptions { include_no_prefix_snippets: true, ..SnippetGetOptions::default() };
            let snippets: Vec<Snippet> = self.read_snippets(Some(&language), path.as_deref(), options).into_iter().filter(Snippet::uses_selection).collect();
            if !snippets.is_empty() {
                let more = json!({ "kind": SURROUND_WITH, "title": "More...", "command": { "id": "editor.action.surroundWithSnippet", "title": "Surround with Snippet..." } });
                let actions = capped(&snippets, more, |snippet| json!({ "title": snippet.name, "kind": SURROUND_WITH, "edit": edit(range, snippet) }));
                lists.push((SURROUND_WITH_SNIPPET_PROVIDER, json!({ "actions": actions })));
            }
        }
        let document = editor.document();
        if document.lines.len() == 1 && document.lines[0].is_empty() {
            let options = SnippetGetOptions { file_template_snippets: Some(true), include_no_prefix_snippets: true, ..SnippetGetOptions::default() };
            let snippets = self.read_snippets(Some(&document.language_id), path.as_deref(), options);
            let full = json!({ "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 1 });
            let more = json!({ "kind": SURROUND_WITH, "title": "Start with Snippet", "command": { "id": "workbench.action.populateFileFromSnippet", "title": "" } });
            let actions = capped(&snippets, more, |snippet| json!({ "title": format!("Start with: {}", snippet.name), "kind": SURROUND_WITH, "edit": edit(&full, snippet) }));
            lists.push((FILE_TEMPLATE_PROVIDER, json!({ "actions": actions })));
        }
        lists
    }
}

impl Workbench {
    /// `editor.snippets.codeActions.enabled`: the providers are registered for every model while it is on.
    pub(super) fn snippet_code_actions_enabled(&self, editor: &crate::editor::OpenEditor) -> bool {
        let overrides = config::configuration::Overrides { resource: Some(&editor.document().path), override_identifier: Some(&editor.document().language_id) };
        self.settings.lock().unwrap().get_value(Some("editor.snippets.codeActions.enabled"), &overrides) == Some(Value::Bool(true))
    }
}

/// Up to `MAX_CODE_ACTIONS` actions, then the overflow action.
fn capped(snippets: &[Snippet], overflow: Value, action: impl Fn(&Snippet) -> Value) -> Vec<Value> {
    let mut actions = Vec::new();
    for snippet in snippets {
        if actions.len() >= MAX_CODE_ACTIONS {
            actions.push(overflow);
            break;
        }
        actions.push(action(snippet));
    }
    actions
}

fn editor_path(editor: &crate::editor::OpenEditor) -> Option<String> {
    (editor.document().scheme == "file").then(|| editor.document().path.to_string_lossy().into_owned())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn empty_snippet() -> Snippet {
    Snippet {
        is_file_template: false,
        scopes: Vec::new(),
        name: String::new(),
        prefix: String::new(),
        description: String::new(),
        body: String::new(),
        source: String::new(),
        snippet_source: SnippetSource::User,
        snippet_identifier: String::new(),
        include: None,
        exclude: None,
    }
}

/// `promoteActiveLanguage`'s `promote`: the pick moves first, its description marked with `*`.
fn promote(picks: &mut Vec<(Item, ConfigurePick)>, index: Option<usize>) {
    let Some(index) = index else { return };
    let (mut item, pick) = picks.remove(index);
    item.description = Some(item.description.map_or_else(|| "*".to_owned(), |description| format!("{description} *")));
    picks.insert(0, (item, pick));
}

/// `createSnippetUri`: a name without `.code-snippets` gets it.
fn snippet_file_path(folder: &Path, name: &str) -> PathBuf {
    match name.ends_with(".code-snippets") {
        true => folder.join(name),
        false => folder.join(format!("{name}.code-snippets")),
    }
}

/// `validateInput` of the snippet file name.
fn snippet_file_name_error(folder: &Path, name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Invalid file name".into());
    }
    if !crate::file_actions::is_valid_basename(name) {
        return Some(format!("'{name}' is not a valid file name"));
    }
    if snippet_file_path(folder, name).exists() {
        return Some(format!("'{name}' already exists"));
    }
    None
}

fn global_snippets_template(scope: &str) -> String {
    [
        "{".to_owned(),
        format!("\t// Place your {scope} snippets here. Each snippet is defined under a snippet name and has a scope, prefix, body and "),
        "\t// description. Add comma separated ids of the languages where the snippet is applicable in the scope field. If scope ".into(),
        "\t// is left empty or omitted, the snippet gets applied to all languages. The prefix is what is ".into(),
        "\t// used to trigger the snippet and the body will be expanded and inserted. Possible variables are: ".into(),
        "\t// $1, $2 for tab stops, $0 for the final cursor position, and ${1:label}, ${2:another} for placeholders. ".into(),
        "\t// Placeholders with the same ids are connected.".into(),
        "\t// Example:".into(),
        "\t// \"Print to console\": {".into(),
        "\t// \t\"scope\": \"javascript,typescript\",".into(),
        "\t// \t\"prefix\": \"log\",".into(),
        "\t// \t\"body\": [".into(),
        "\t// \t\t\"console.log('$1');\",".into(),
        "\t// \t\t\"$2\"".into(),
        "\t// \t],".into(),
        "\t// \t\"description\": \"Log output to console\"".into(),
        "\t// }".into(),
        "\t//".into(),
        "\t// You can also restrict snippets to specific files using include/exclude patterns:".into(),
        "\t// \"Test snippet\": {".into(),
        "\t// \t\"scope\": \"javascript,typescript\",".into(),
        "\t// \t\"prefix\": \"test\",".into(),
        "\t// \t\"body\": \"test('$1', () => {\\n\\t$0\\n});\",".into(),
        "\t// \t\"include\": [\"**/*.test.ts\", \"*.spec.ts\"],".into(),
        "\t// \t\"exclude\": [\"**/temp/*.ts\"],".into(),
        "\t// \t\"description\": \"Insert test block\"".into(),
        "\t// }".into(),
        "}".into(),
    ]
    .join("\n")
}

fn language_snippets_template(label: &str) -> String {
    [
        "{".to_owned(),
        format!("\t// Place your snippets for {label} here. Each snippet is defined under a snippet name and has a prefix, body and "),
        "\t// description. The prefix is what is used to trigger the snippet and the body will be expanded and inserted. Possible variables are:".into(),
        "\t// $1, $2 for tab stops, $0 for the final cursor position, and ${1:label}, ${2:another} for placeholders. Placeholders with the ".into(),
        "\t// same ids are connected.".into(),
        "\t// Example:".into(),
        "\t// \"Print to console\": {".into(),
        "\t// \t\"prefix\": \"log\",".into(),
        "\t// \t\"body\": [".into(),
        "\t// \t\t\"console.log('$1');\",".into(),
        "\t// \t\t\"$2\"".into(),
        "\t// \t],".into(),
        "\t// \t\"description\": \"Log output to console\"".into(),
        "\t// }".into(),
        "\t//".into(),
        "\t// You can also restrict snippets to specific files using include/exclude patterns:".into(),
        "\t// \"Test snippet\": {".into(),
        "\t// \t\"prefix\": \"test\",".into(),
        "\t// \t\"body\": \"test('$1', () => {\\n\\t$0\\n});\",".into(),
        "\t// \t\"include\": [\"**/*.test.ts\", \"*.spec.ts\"],".into(),
        "\t// \t\"exclude\": [\"**/temp/*.ts\"],".into(),
        "\t// \t\"description\": \"Insert test block\"".into(),
        "\t// }".into(),
        "}".into(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use config::registry::Registry;
    use config::service::Settings;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    /// A folder with `a.txt` holding `word`, a user `plaintext.json` with a surrounding and a plain snippet.
    fn open(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-snippet-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("User/snippets")).unwrap();
        std::fs::write(dir.join("a.txt"), "word").unwrap();
        std::fs::write(dir.join("User/snippets/plaintext.json"), r#"{ "Bold": { "prefix": "b", "body": "<b>$TM_SELECTED_TEXT</b>" }, "Plain": { "prefix": "p", "body": "plain" }, "Wrap": { "prefix": "word", "body": "[$0]" } }"#).unwrap();
        let workspace = config::workspace::Workspace::folder(dir.clone());
        let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.watch_editor_focus(window, cx);
            workbench.open_file(dir.join("a.txt"), true, window, cx);
            window.focus(&workbench.editor_focus, cx);
            workbench
        });
        cx.run_until_parked();
        (workbench, cx)
    }

    fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str, args: Vec<Value>) {
        workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, args, window, cx));
        cx.run_until_parked();
    }

    fn text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
        workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"))
    }

    /// insertSnippet.ts by name and by body, and the Surround With code actions of a selection.
    #[gpui::test]
    fn snippets_insert_by_name_or_body_and_surround_the_selection(cx: &mut TestAppContext) {
        let (workbench, cx) = open(cx);
        run(&workbench, cx, "editor.action.selectAll", vec![]);
        let actions = workbench.update(cx, |workbench, _| {
            workbench.refresh_snippet_files();
            let editor = workbench.editors.active().unwrap();
            let range = json!({ "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 5 });
            workbench.snippet_code_actions(editor, &range)
        });
        assert_eq!(actions.len(), 1);
        assert_eq!((actions[0].0, actions[0].1["actions"][0]["title"].as_str()), (SURROUND_WITH_SNIPPET_PROVIDER, Some("Bold")));
        run(&workbench, cx, "editor.action.insertSnippet", vec![json!({ "name": "Bold" })]);
        assert_eq!(text(&workbench, cx), "<b>word</b>");
        run(&workbench, cx, "editor.action.insertSnippet", vec![json!({ "snippet": " and ${1:more}" })]);
        assert_eq!(text(&workbench, cx), "<b>word</b> and more");
    }

    /// tabCompletion.ts: a selection equal to a prefix offers that snippet, and Tab inserts it over the selection.
    #[gpui::test]
    fn tab_completes_the_snippet_whose_prefix_is_selected(cx: &mut TestAppContext) {
        let (workbench, cx) = open(cx);
        run(&workbench, cx, "editor.action.selectAll", vec![]);
        let suggestions = workbench.update(cx, |workbench, _| {
            workbench.refresh_snippet_files();
            workbench.tab_completion_suggestions()
        });
        assert_eq!(suggestions["b"].as_array().map(Vec::len), Some(1));
        assert_eq!(suggestions["b"][0]["j"]["startColumn"], json!(1));
        run(&workbench, cx, "insertSnippet", vec![]);
        assert_eq!(text(&workbench, cx), "[]");
    }

    #[test]
    fn snippet_file_names_get_the_extension_and_are_checked() {
        let folder = std::env::temp_dir().join(format!("snippet-names-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("taken.code-snippets"), "{}").unwrap();
        assert_eq!(snippet_file_path(&folder, "mine"), folder.join("mine.code-snippets"));
        assert_eq!(snippet_file_path(&folder, "mine.code-snippets"), folder.join("mine.code-snippets"));
        assert_eq!(snippet_file_name_error(&folder, ""), Some("Invalid file name".into()));
        assert_eq!(snippet_file_name_error(&folder, "a/b"), Some("'a/b' is not a valid file name".into()));
        assert_eq!(snippet_file_name_error(&folder, "taken"), Some("'taken' already exists".into()));
        assert_eq!(snippet_file_name_error(&folder, "mine"), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn snippets_know_the_variables_they_read() {
        let snippet = |body: &str| Snippet { body: body.into(), ..empty_snippet() };
        assert!(snippet("<b>$TM_SELECTED_TEXT</b>").uses_selection());
        assert!(snippet("(${1:$SELECTION})").uses_selection());
        assert!(!snippet("${CLIPBOARD:$SELECTION}").uses_selection());
        assert!(snippet("${CLIPBOARD:x}").needs_clipboard());
        assert!(!snippet("$1 text").needs_clipboard());
    }
}
