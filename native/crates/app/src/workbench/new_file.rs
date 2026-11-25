//! welcomeViews/common/newFile.contribution.ts: File > New File... (`welcome.showNewFileEntries`) lists the
//! `MenuId.NewFile` items, the built-in Text File and what extensions add to `file/newFile`, by group; a typed name
//! adds Create New File (name).

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::menus::{Entry as MenuEntry, MenuId};
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};

const BUILT_IN: &str = "Built-In";

/// `NewFileItem`.
#[derive(Clone, Debug, PartialEq)]
pub struct NewFileItem {
    command: String,
    args: Option<Value>,
    title: String,
    from: String,
    group: String,
}

/// `Owner::NewFile`: the entries as listed, by item handle.
pub struct NewFilePicker {
    shown: Vec<NewFileItem>,
    entries: Vec<NewFileItem>,
}

impl Workbench {
    pub(super) fn register_new_file_commands(&mut self) {
        self.commands.register("welcome.showNewFileEntries", |workbench, _, window, cx| {
            let entries = workbench.new_file_entries(window);
            if let [entry] = entries.as_slice() {
                return workbench.execute_command(&entry.command.clone(), Vec::new(), window, cx);
            }
            workbench.show_new_file_picker(entries, window, cx);
            Outcome::done()
        });
    }

    /// `allEntries`: the `MenuId.NewFile` actions whose `when` holds, with their short titles and sources.
    fn new_file_entries(&self, window: &Window) -> Vec<NewFileItem> {
        let context = self.key_context(window);
        let mut items: Vec<_> = self.menus.iter().filter(|item| item.menu == MenuId::NewFile && item.when.as_ref().is_none_or(|when| when.evaluate(&context))).collect();
        items.sort_by(|a, b| a.group.cmp(&b.group).then(a.order.total_cmp(&b.order)));
        let mut entries = Vec::new();
        for item in items {
            let MenuEntry::Command { id, .. } = &item.entry else { continue };
            let contribution = self.extensions.iter().find_map(|extension| {
                let commands = extension.description["contributes"]["commands"].as_array()?;
                let command = commands.iter().find(|command| command["command"] == json!(id))?;
                let name = extension.description["displayName"].as_str().or(extension.description["name"].as_str()).unwrap_or_default();
                Some((name.to_owned(), command["shortTitle"].as_str().map(str::to_owned)))
            });
            let (from, short_title) = contribution.unwrap_or((BUILT_IN.into(), None));
            entries.push(NewFileItem { command: id.clone(), args: None, title: short_title.unwrap_or_else(|| item.title.replace("&&", "")), from, group: item.group.clone() });
        }
        entries
    }

    fn show_new_file_picker(&mut self, entries: Vec<NewFileItem>, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("New File...".into());
        input.placeholder = Some("Select File Type or Enter File Name...".into());
        input.sort_by_label = false;
        input.match_on_description = true;
        input.match_on_detail = true;
        let shown = sorted(entries.clone());
        input.set_items(self.new_file_items(&shown, window));
        let id = self.quick_inputs.add(input, Owner::NewFile(NewFilePicker { shown, entries }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `refreshQp`: a separator per group, each entry with its keybinding, source and Configure Keybinding button.
    fn new_file_items(&self, entries: &[NewFileItem], window: &Window) -> Vec<Entry> {
        let context = self.key_context(window);
        let mut items = Vec::new();
        let mut last_group = None;
        for (handle, entry) in entries.iter().enumerate() {
            if last_group != Some(&entry.group) {
                let label = match entry.group.as_str() {
                    "file" => "File".to_owned(),
                    "notebook" => "Notebook".to_owned(),
                    group => group.to_owned(),
                };
                items.push(Entry::Separator(Some(label)));
                last_group = Some(&entry.group);
            }
            let keybinding = self.keybindings.resolver.lookup_primary_keybinding(&entry.command, &context);
            items.push(Entry::Item(Item {
                handle: handle as i64,
                label: entry.title.clone(),
                description: Some(entry.from.clone()),
                detail: Some(String::new()),
                keybinding: keybinding.map(|item| item.keybinding.iter().map(|chord| chord.key_caps()).collect()).unwrap_or_default(),
                buttons: vec![Button { handle: 0, codicon: Some("gear".into()), tooltip: Some("Configure Keybinding".into()), ..Button::default() }],
                ..Item::default()
            }));
        }
        items
    }

    pub fn new_file_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::NewFile(picker)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::ChangeValue(value) => {
                let mut entries = picker.entries.clone();
                if !value.is_empty() {
                    let typed = NewFileItem { command: "workbench.action.files.newFile".into(), args: Some(json!({ "languageId": null, "viewType": null, "fileName": value })), title: format!("Create New File ({value})"), group: "file".into(), from: BUILT_IN.into() };
                    entries.insert(0, typed);
                }
                let shown = sorted(entries);
                let items = self.new_file_items(&shown, window);
                if let Some(Owner::NewFile(picker)) = self.quick_inputs.owner_mut(id) {
                    picker.shown = shown;
                }
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.set_items(items);
                }
                self.flush_quick_input_events(window, cx);
            }
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                let entry = picker.shown[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                let outcome = self.execute_command(&entry.command, entry.args.into_iter().collect(), window, cx);
                super::quick_access::show_failure(entry.title, outcome, window, cx);
            }
            Event::TriggerItemButton(handle, ..) => {
                let command = picker.shown[handle as usize].command.clone();
                self.hide_quick_input_by_id(id, window, cx);
                let outcome = self.execute_command("workbench.action.openGlobalKeybindings", vec![json!(command)], window, cx);
                super::quick_access::show_failure("Preferences: Open Keyboard Shortcuts".into(), outcome, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

/// `entries.sort((a, b) => -sortCategories(a, b))`: file first, then notebook, then other groups; within a group the
/// built-in entries, then extensions by name, descending.
fn sorted(mut entries: Vec<NewFileItem>) -> Vec<NewFileItem> {
    let priority = |group: &str| match group {
        "file" => Some(1),
        "notebook" => Some(2),
        _ => None,
    };
    let compare = |a: &NewFileItem, b: &NewFileItem| -> i32 {
        match (priority(&a.group), priority(&b.group)) {
            (Some(a), Some(b)) if a != b => return b - a,
            (Some(_), None) => return 1,
            (None, Some(_)) => return -1,
            _ => {}
        }
        if a.from == BUILT_IN {
            return 1;
        }
        if b.from == BUILT_IN {
            return -1;
        }
        match crate::command_palette::locale_compare(&a.from, &b.from) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    };
    entries.sort_by(|a, b| (-compare(a, b)).cmp(&0));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, from: &str, group: &str) -> NewFileItem {
        NewFileItem { command: title.into(), args: None, title: title.into(), from: from.into(), group: group.into() }
    }

    #[gpui::test]
    fn with_only_text_file_new_file_opens_an_untitled_editor(cx: &mut gpui::TestAppContext) {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        let (entries, scheme) = workbench.update_in(cx, |workbench, window, cx| {
            let entries = workbench.new_file_entries(window);
            drop(workbench.execute_command("welcome.showNewFileEntries", Vec::new(), window, cx));
            (entries, workbench.editors.active().map(|editor| editor.document.scheme.clone()))
        });
        assert_eq!(entries, vec![NewFileItem { command: "workbench.action.files.newUntitledFile".into(), args: None, title: "Text File".into(), from: BUILT_IN.into(), group: "file".into() }]);
        assert_eq!(scheme.as_deref(), Some("untitled"));
    }

    #[test]
    fn files_come_first_then_notebooks_with_built_ins_first_in_a_group() {
        let entries = vec![item("Jupyter", ".ipynb Support", "notebook"), item("Text File", BUILT_IN, "file"), item("Typed", "A", "file"), item("Other", "B", "zz")];
        let titles: Vec<String> = sorted(entries).into_iter().map(|entry| entry.title).collect();
        assert_eq!(titles, ["Text File", "Typed", "Jupyter", "Other"]);
    }
}
