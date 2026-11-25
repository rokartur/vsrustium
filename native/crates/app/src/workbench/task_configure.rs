//! abstractTaskService.ts: `_trust`, Configure Task (`_runConfigureTasks`), Configure Default Build and Test Task, and
//! `customize`, which writes a task's customization into the folder's tasks.json.

use std::path::{Path, PathBuf};

use config::service::Target;
use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Map, Value, json};

use super::Workbench;
use super::task_quick_pick::{PickAction, TaskPick, TaskPicker, push_entry};
use crate::commands::Outcome;
use crate::editor::{Position, Selection, VerticalReveal};
use crate::main_thread_quick_open::Owner;
use crate::notifications::{Notification, Severity};
use crate::quick_input::{Entry, Item, Kind, QuickInput};
use crate::tasks::configuration::{CUSTOMIZED_TASK_TYPE, Task};

/// `TaskService.requestTrust`.
const TRUST_MESSAGE: &str = "Listing and running tasks requires that some of the files in this workspace be executed as code.";
/// The `tasksJsonComment` a new tasks.json starts with.
const TASKS_JSON_COMMENT: &str = "\t// See https://go.microsoft.com/fwlink/?LinkId=733558 \n\t// for the documentation about the tasks.json format";

impl Workbench {
    /// `_trust`: in an untrusted workspace the workspace trust request comes first; `run` only once trusted.
    pub(super) fn with_task_trust(&mut self, window: &mut Window, cx: &mut Context<Self>, run: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> Outcome + 'static) -> Outcome {
        if self.workspace_trust.is_trusted() {
            return run(self, window, cx);
        }
        let answer = match self.request_workspace_trust(&json!({ "message": TRUST_MESSAGE }), window, cx) {
            Outcome::Ready(Ok(Value::Bool(true))) => return run(self, window, cx),
            Outcome::Ready(_) => return Outcome::done(),
            Outcome::Pending(answer) => answer,
        };
        let (reply, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            if !matches!(answer.await, Ok(Ok(Value::Bool(true)))) {
                let _ = reply.send(Ok(Value::Null));
                return;
            }
            let outcome = workbench.update_in(cx, |workbench, window, cx| run(workbench, window, cx));
            let result = match outcome {
                Ok(Outcome::Ready(result)) => result,
                Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(Value::Null)),
                Err(_) => Ok(Value::Null),
            };
            let _ = reply.send(result);
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_runConfigureTasks`: every task by label, then Create or Open tasks.json when no task is configured, then
    /// the detection entries of grunt, gulp and jake.
    pub(super) fn run_configure_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fetched = self.fetch_tasks(None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let mut tasks = fetched.await.unwrap_or_default();
            tasks.sort_by(|a, b| crate::command_palette::locale_compare(&a.label, &b.label));
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.show_configure_tasks(tasks, window, cx));
        })
        .detach();
    }

    fn show_configure_tasks(&mut self, tasks: Vec<Task>, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        let configured = tasks.iter().filter(|task| !is_contributed(task)).count();
        for task in tasks {
            let mut item = self.task_item(&task);
            item.buttons.clear();
            push_entry(&mut entries, &mut picks, item, TaskPick::Task(task));
        }
        let needs_create_or_open = configured == 0;
        let folder = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        if needs_create_or_open && let Some(folder) = folder {
            let exists = folder.join(".vscode/tasks.json").exists();
            let label = if exists { "Open tasks.json file" } else { "Create tasks.json file from template" };
            if !entries.is_empty() {
                entries.push(Entry::Separator(None));
            }
            push_entry(&mut entries, &mut picks, Item { label: label.into(), ..Item::default() }, TaskPick::OpenConfig(folder));
        }
        // `QUICKOPEN_SKIP_CONFIG`: a single task is configured without the picker.
        if picks.len() == 1 && !needs_create_or_open && self.setting("task.quickOpen.skip") == true {
            let TaskPick::Task(task) = picks.remove(0) else { return };
            return self.configure_task(task, window, cx);
        }
        for task_type in ["grunt", "gulp", "jake"] {
            if self.setting(&format!("{task_type}.autoDetect")) == "off" {
                let mut name = task_type.to_owned();
                name[..1].make_ascii_uppercase();
                let label = format!("$(gear) {name} task detection is turned off. Enable {task_type} task detection...");
                push_entry(&mut entries, &mut picks, Item { label, always_show: true, ..Item::default() }, TaskPick::EnableDetection(task_type.into()));
            }
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a task to configure".into());
        input.match_on_description = true;
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Tasks(TaskPicker { picks, action: PickAction::Configure }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `_configureTask`: a contributed task is customized, a tasks.json task opened at its entry.
    pub(super) fn configure_task(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        if is_contributed(&task) {
            self.customize_task(&task, None, true, window, cx);
        } else if !task.configures {
            self.open_task_entry(&task, window, cx);
        }
    }

    /// `_runConfigureDefaultBuildTask` and `_runConfigureDefaultTestTask`: the current default first, marked; the
    /// picked task becomes the group's default and the old one stays in the group.
    pub(super) fn configure_default_task(&mut self, group: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let fetched = self.fetch_tasks(None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let tasks = fetched.await.unwrap_or_default();
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if tasks.is_empty() {
                    return workbench.run_configure_tasks(window, cx);
                }
                workbench.show_default_task_picker(group, tasks, window, cx);
            });
        })
        .detach();
    }

    fn show_default_task_picker(&mut self, group: &'static str, mut tasks: Vec<Task>, window: &mut Window, cx: &mut Context<Self>) {
        let selected = default_task(&tasks, group);
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        if let Some(selected) = &selected {
            let label = match group {
                "build" => format!("{} is already marked as the default build task", selected.label),
                _ => format!("{} is already marked as the default test task.", selected.label),
            };
            let item = Item { label, ..self.task_item(selected) };
            push_entry(&mut entries, &mut picks, Item { buttons: Vec::new(), ..item }, TaskPick::Task(selected.clone()));
        }
        // The test picker goes through `_showQuickPick`, which sorts; the build picker keeps the fetch order.
        if group == "test" {
            tasks.sort_by(|a, b| crate::command_palette::locale_compare(&a.label, &b.label));
        }
        for task in tasks.into_iter().filter(|task| Some(&task.id) != selected.as_ref().map(|selected| &selected.id)) {
            let mut item = self.task_item(&task);
            item.buttons.clear();
            push_entry(&mut entries, &mut picks, item, TaskPick::Task(task));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(format!("Select the task to be used as the default {group} task"));
        input.match_on_description = true;
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Tasks(TaskPicker { picks, action: PickAction::ConfigureDefault(group) }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// The picked default: `customize(task, { group: { kind, isDefault: true } }, true)`, then the old default
    /// back to `group: kind`.
    pub(super) fn set_default_task(&mut self, group: &'static str, task: Task, all: Vec<Task>, window: &mut Window, cx: &mut Context<Self>) {
        let selected = default_task(&all, group);
        let was_selected = selected.as_ref().is_some_and(|selected| selected.id == task.id);
        if was_selected && !is_contributed(&task) {
            self.open_task_entry(&task, window, cx);
        }
        self.customize_task(&task, Some(json!({ "kind": group, "isDefault": true })), true, window, cx);
        if let Some(selected) = selected.filter(|_| !was_selected) {
            // The first write changed tasks.json: the old default is read again from it.
            let fresh = self.configured_tasks().into_iter().find(|candidate| candidate.label == selected.label).unwrap_or(selected);
            self.customize_task(&fresh, Some(json!(group)), false, window, cx);
        }
    }

    /// `customize`: the task's tasks.json entry (a new one for a contributed task) with `group` set, written back
    /// as `tasks.tasks`, or a new tasks.json holding only it.
    pub(super) fn customize_task(&mut self, task: &Task, group: Option<Value>, open_config: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = task.workspace_folder.clone() else { return };
        let (file_config, has_parse_errors) = self.folder_tasks_config(&folder);
        if has_parse_errors {
            self.notify(Notification::new(Severity::Warning, "The current task configuration has errors. Please fix the errors first before customizing a task."));
            return;
        }
        let (index, mut to_customize) = customizable_task(task, file_config.as_ref());
        if let Some(group) = group {
            to_customize.insert("group".into(), group);
        }
        let to_customize = Value::Object(to_customize);
        let path = match file_config {
            None => {
                let path = folder.join(".vscode").join("tasks.json");
                let content = new_tasks_json(&to_customize, self.setting("editor.insertSpaces") == true, self.setting("editor.tabSize").as_u64().unwrap_or(4) as usize);
                if let Err(error) = std::fs::create_dir_all(folder.join(".vscode")).and_then(|()| std::fs::write(&path, content)) {
                    return self.notify_error(format!("Unable to write {}: {error}", path.display()));
                }
                self.reload_task_settings();
                path
            }
            Some(file_config) => {
                let mut tasks = file_config["tasks"].as_array().cloned().unwrap_or_default();
                match index {
                    Some(index) => tasks[index] = to_customize.clone(),
                    None => tasks.push(to_customize.clone()),
                }
                let multi_root = self.workspace().is_some_and(|workspace| workspace.file.is_some());
                let target = if multi_root { Target::WorkspaceFolder } else { Target::Workspace };
                let mut settings = self.settings.lock().unwrap();
                let change = settings.update_value("tasks.tasks", Some(&Value::Array(tasks)), target, Some(&folder), &[]);
                match change {
                    Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
                    Ok(_) => {}
                    Err(error) => {
                        drop(settings);
                        return self.notify_error(error);
                    }
                }
                config::service::folder_config_dir(&folder).join("tasks.json")
            }
        };
        if open_config {
            self.open_tasks_json_at(path, &to_customize, window, cx);
        }
    }

    /// `openConfig`: the folder's tasks.json at the task's entry.
    fn open_task_entry(&mut self, task: &Task, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = task.workspace_folder.clone() else { return };
        let (file_config, _) = self.folder_tasks_config(&folder);
        let (Some(index), _) = customizable_task(task, file_config.as_ref()) else { return };
        let element = file_config.expect("an index comes from the file")["tasks"][index].clone();
        self.open_tasks_json_at(config::service::folder_config_dir(&folder).join("tasks.json"), &element, window, cx);
    }

    /// The folder's `tasks` configuration (tasks.json), and whether its file has parse errors.
    fn folder_tasks_config(&self, folder: &Path) -> (Option<Value>, bool) {
        let inspected = self.settings.lock().unwrap().configuration.inspect("tasks", &config::configuration::Overrides { resource: Some(folder), override_identifier: None });
        let file = config::service::folder_config_dir(folder).join("tasks.json");
        let has_parse_errors = std::fs::read_to_string(&file).is_ok_and(|content| {
            let mut errors = Vec::new();
            config::json::parse(&content, &mut errors, config::json::ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..Default::default() });
            !errors.is_empty()
        });
        (inspected.workspace_folder.value.or(inspected.workspace.value), has_parse_errors)
    }

    fn reload_task_settings(&mut self) {
        let mut settings = self.settings.lock().unwrap();
        if let Some(change) = settings.reload_changed_files() {
            self.host.send_configuration_change(&settings, &change);
        }
    }

    /// `_openEditorAtTask`: the entry found by its formatted text, its lines selected.
    fn open_tasks_json_at(&mut self, path: PathBuf, element: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let selection = entry_selection(&content, element, self.setting("editor.tabSize").as_u64().unwrap_or(4) as usize);
        self.open_file(path, false, window, cx);
        let Some((start, end)) = selection else { return };
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        editor.set_selections(vec![Selection { anchor: start, head: end }]);
        editor.reveal_range(start, end, VerticalReveal::CenterIfOutsideViewport, viewport);
        self.editor_scrolled(cx);
    }
}

/// A task an extension provided, not a tasks.json task (`ContributedTask.is`).
fn is_contributed(task: &Task) -> bool {
    task.source["label"] != "Workspace"
}

/// The group's default: the one default build task (`_getDefaultTasks`), or the first default test task.
fn default_task(tasks: &[Task], group: &str) -> Option<Task> {
    let mut defaults = tasks.iter().filter(|task| task.group.as_ref().is_some_and(|value| value["_id"] == group && value["isDefault"] == true));
    match group {
        "build" => {
            let first = defaults.next()?;
            defaults.next().is_none().then(|| first.clone())
        }
        _ => defaults.next().cloned(),
    }
}

/// `_createCustomizableTask`: the tasks.json entry of a configured task with its index, or a new entry with a
/// contributed task's identifier, matchers and group.
pub(super) fn customizable_task(task: &Task, file_config: Option<&Value>) -> (Option<usize>, Map<String, Value>) {
    let entries = file_config.and_then(|config| config["tasks"].as_array()).cloned().unwrap_or_default();
    let identifier: Map<String, Value> = task.definition.as_object().into_iter().flatten().filter(|(key, _)| *key != "_key").map(|(key, value)| (key.clone(), value.clone())).collect();
    let index = match is_contributed(task) {
        true => None,
        false if task.definition["type"] == CUSTOMIZED_TASK_TYPE => entries.iter().position(|entry| entry["label"].as_str().or(entry["taskName"].as_str()) == Some(task.name.as_str())),
        false => entries.iter().position(|entry| identifier.iter().all(|(key, value)| entry.get(key) == Some(value))),
    };
    let mut element = match index {
        Some(index) => entries[index].as_object().cloned().unwrap_or_default(),
        None => {
            let mut element = identifier;
            if !task.problem_matchers.is_empty() && task.problem_matchers.iter().all(Value::is_string) {
                element.insert("problemMatcher".into(), Value::Array(task.problem_matchers.clone()));
            }
            if let Some(group) = &task.group {
                element.insert("group".into(), group_kind(group));
            }
            element
        }
    };
    if task.problem_matchers.is_empty() && !element.contains_key("problemMatcher") || task.problem_matchers.is_empty() && index.is_none() {
        element.insert("problemMatcher".into(), json!([]));
    }
    element.insert("label".into(), json!(task.label));
    match &task.detail {
        Some(detail) => element.insert("detail".into(), json!(detail)),
        None => element.remove("detail"),
    };
    (index, element)
}

/// `GroupKind.to`: the group's kind, or `{ kind, isDefault }` for the default.
fn group_kind(group: &Value) -> Value {
    match group["isDefault"] {
        Value::Bool(true) => json!({ "kind": group["_id"], "isDefault": true }),
        _ => group["_id"].clone(),
    }
}

/// The content of a new tasks.json holding `task`, tabs turned into spaces with `editor.insertSpaces`.
pub(super) fn new_tasks_json(task: &Value, insert_spaces: bool, tab_size: usize) -> String {
    let value = json!({ "version": "2.0.0", "tasks": [task] });
    let content = format!("{{\n{TASKS_JSON_COMMENT}{}", &stringify_tabs(&value)[1..]);
    if !insert_spaces {
        return content;
    }
    let indent = regex::Regex::new(r"(\n)(\t+)").unwrap();
    indent.replace_all(&content, |captures: &regex::Captures| format!("\n{}", " ".repeat(captures[2].len() * tab_size))).into_owned()
}

/// `JSON.stringify(value, null, '\t')`.
fn stringify_tabs(value: &Value) -> String {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(value, &mut serializer).expect("JSON values serialize");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// `_formatTaskForJson` and the selection of `_openEditorAtTask`: the entry at three levels of the file's indent,
/// its first line selected from column 4 when it is one line, else its lines from column 3.
pub(super) fn entry_selection(content: &str, element: &Value, tab_size: usize) -> Option<(Position, Position)> {
    let insert_spaces = !content.contains("\n\t");
    let unit = if insert_spaces { " ".repeat(tab_size) } else { "\t".into() };
    let mut stringified = stringify_tabs(element);
    if insert_spaces {
        stringified = regex::Regex::new(r"(\n)(\t+)").unwrap().replace_all(&stringified, |captures: &regex::Captures| format!("\n{}", " ".repeat(captures[2].len() * tab_size))).into_owned();
    }
    let stringified = stringified.replace(&format!("\n{unit}"), &format!("\n{}", unit.repeat(3)));
    let two = unit.repeat(2);
    let (body, last) = stringified.split_at(stringified.len() - 1);
    let string_value = format!("{two}{body}{two}{last}");
    let index = content.find(&string_value)?;
    let start_line = content[..index].matches('\n').count();
    let end_line = start_line + string_value.matches('\n').count();
    if start_line == 0 {
        return None;
    }
    let single = start_line == end_line;
    let start = Position { line: start_line, column: if single { 3 } else { 2 } };
    let end = Position { line: end_line, column: if single { content.lines().nth(end_line).map_or(0, |line| line.chars().count()) } else { 3 } };
    Some((start, end))
}

#[cfg(test)]
mod tests;
