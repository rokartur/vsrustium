//! taskQuickPick.ts and the pickers of abstractTaskService.ts: run (two levels: recent and configured tasks, then
//! the tasks of a contributed type), build and test groups, terminate and restart, and Configure Task.

use std::path::{Path, PathBuf};

use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::dialog;
use crate::main_thread_quick_open::Owner;
use crate::notifications::{Notification, Severity};
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};
use crate::tasks::configuration::{CUSTOMIZED_TASK_TYPE, Task};

/// `AbstractTaskService.RecentlyUsedTasks_KeyV2`, workspace storage.
const RECENT_TASKS_KEY: &str = "workbench.tasks.recentlyUsedTasks2";
const SHOW_ALL: &str = "Show All Tasks...";
const CONFIGURE_TOOLTIP: &str = "Configure Task";
const REMOVE_RECENT_TOOLTIP: &str = "Remove Recently Used Task";

#[derive(Clone)]
pub enum TaskPick {
    Task(Task),
    /// A contributed type: its tasks on the second level.
    Type(String),
    ShowAll,
    /// The default entry (`task: null`): configure tasks, or go back on the second level.
    Configure,
    GoBack,
    /// `settingType`: `{type}.autoDetect` is off.
    EnableDetection(String),
    TerminateAll,
    /// One running instance of a task, by its terminal.
    Instance(Task, u32),
    /// Configure Task's Create or Open tasks.json entry of a folder.
    OpenConfig(PathBuf),
    /// "No build task to run found. Configure Build Task...": Configure Default Build or Test Task.
    ConfigureDefault(&'static str),
    Nothing,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PickAction {
    Run,
    Terminate,
    Restart,
    /// Configure Task: the picked task is customized or opened.
    Configure,
    /// Configure Default Build or Test Task.
    ConfigureDefault(&'static str),
}

#[derive(Clone)]
pub struct TaskPicker {
    pub picks: Vec<TaskPick>,
    pub action: PickAction,
}

impl Workbench {
    pub(super) fn register_task_pick_actions(&mut self) {
        // abstractTaskService.ts `_registerCommands`: each command asks for workspace trust first (`_trust`).
        self.commands.register("workbench.action.tasks.build", |workbench, _, window, cx| workbench.with_task_trust(window, cx, |workbench, window, cx| workbench.run_group_command("build", window, cx)));
        self.commands.register("workbench.action.tasks.test", |workbench, _, window, cx| workbench.with_task_trust(window, cx, |workbench, window, cx| workbench.run_group_command("test", window, cx)));
        self.commands.register("workbench.action.tasks.terminate", |workbench, args, window, cx| {
            let argument = args.into_iter().next().unwrap_or(Value::Null);
            workbench.with_task_trust(window, cx, move |workbench, window, cx| {
                workbench.terminate_task_command(argument, window, cx);
                Outcome::done()
            })
        });
        self.commands.register("workbench.action.tasks.restartTask", |workbench, args, window, cx| {
            let argument = args.into_iter().next().unwrap_or(Value::Null);
            workbench.with_task_trust(window, cx, move |workbench, window, cx| {
                workbench.restart_task_command(argument, window, cx);
                Outcome::done()
            })
        });
        self.commands.register("workbench.action.tasks.reRunTask", |workbench, _, window, cx| {
            workbench.with_task_trust(window, cx, |workbench, window, cx| {
                match workbench.tasks.last.clone() {
                    Some(task) => drop(workbench.run_task(task, window, cx)),
                    None => workbench.show_run_task_picker(None, window, cx),
                }
                Outcome::done()
            })
        });
        self.commands.register("workbench.action.tasks.showTasks", |workbench, _, window, cx| {
            workbench.with_task_trust(window, cx, |workbench, window, cx| {
                workbench.terminate_or_show(PickAction::Restart, "Select the task to show its output", window, cx);
                Outcome::done()
            })
        });
        self.commands.register("workbench.action.tasks.configureTaskRunner", |workbench, _, window, cx| {
            workbench.with_task_trust(window, cx, |workbench, window, cx| {
                workbench.run_configure_tasks(window, cx);
                Outcome::done()
            })
        });
        self.commands.register("workbench.action.tasks.configureDefaultBuildTask", |workbench, _, window, cx| {
            workbench.with_task_trust(window, cx, |workbench, window, cx| {
                workbench.configure_default_task("build", window, cx);
                Outcome::done()
            })
        });
        self.commands.register("workbench.action.tasks.configureDefaultTestTask", |workbench, _, window, cx| {
            workbench.with_task_trust(window, cx, |workbench, window, cx| {
                workbench.configure_default_task("test", window, cx);
                Outcome::done()
            })
        });
    }

    /// `_doRunTaskCommand` with the two-level picker; `name` fills the filter.
    pub(super) fn show_run_task_picker(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let configured: Vec<Task> = self.configured_tasks().into_iter().filter(|task| !task.hide).collect();
        // `QUICKOPEN_SKIP_CONFIG`: a single configured task runs without the picker.
        if configured.len() == 1 && self.setting("task.quickOpen.skip") == true {
            self.run_picked_task(configured[0].clone(), window, cx);
            return;
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select the task to run".into());
        input.match_on_description = true;
        let picker = self.top_level_entries(&mut input, configured);
        let id = self.quick_inputs.add(input, Owner::Tasks(picker));
        if let Some(name) = name {
            self.quick_inputs.get_mut(id).expect("just added").set_value(name);
        }
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `getTopLevelEntries`: recently used, configured (or Configure a Task), then the contributed types.
    fn top_level_entries(&mut self, input: &mut QuickInput, configured: Vec<Task>) -> TaskPicker {
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        let recent = self.recent_tasks(&configured);
        let remove = Button { handle: 1, codicon: Some("close".into()), tooltip: Some(REMOVE_RECENT_TOOLTIP.into()), ..Button::default() };
        if !recent.is_empty() {
            self.task_group_entries(&mut entries, &mut picks, "recently used", recent.clone(), Some(remove));
        }
        let mut deduped: Vec<Task> = configured.iter().filter(|task| !recent.iter().any(|seen| recent_key(seen) == recent_key(task))).cloned().collect();
        deduped.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        if !deduped.is_empty() {
            self.task_group_entries(&mut entries, &mut picks, "configured", deduped, None);
        }
        if configured.is_empty() {
            entries.push(Entry::Separator(Some("configured".into())));
            push_entry(&mut entries, &mut picks, Item { label: "$(plus) Configure a Task".into(), ..Item::default() }, TaskPick::Configure);
        }
        let mut types: Vec<String> = Vec::new();
        for (_, task_type) in &self.tasks.providers {
            if !types.contains(task_type) {
                types.push(task_type.clone());
            }
        }
        if !types.is_empty() {
            entries.push(Entry::Separator(Some("contributed".into())));
            for task_type in types {
                push_entry(&mut entries, &mut picks, Item { label: format!("$(folder) {task_type}"), ..Item::default() }, TaskPick::Type(task_type));
            }
            push_entry(&mut entries, &mut picks, Item { label: SHOW_ALL.into(), always_show: true, ..Item::default() }, TaskPick::ShowAll);
        }
        input.set_items(entries);
        TaskPicker { picks, action: PickAction::Run }
    }

    /// `_createEntriesForGroup`: a separator, then each task with its Configure Task button.
    fn task_group_entries(&self, entries: &mut Vec<Entry>, picks: &mut Vec<TaskPick>, label: &str, tasks: Vec<Task>, extra: Option<Button>) {
        entries.push(Entry::Separator(Some(label.into())));
        for task in tasks {
            let mut item = self.task_item(&task);
            item.buttons.extend(extra.clone());
            push_entry(entries, picks, item, TaskPick::Task(task));
        }
    }

    /// `_createTaskEntry`: the label with the task's icon, the folder in a multi-root workspace, the detail.
    pub(super) fn task_item(&self, task: &Task) -> Item {
        let configure = Button { handle: 0, codicon: Some("gear".into()), tooltip: Some(CONFIGURE_TOOLTIP.into()), ..Button::default() };
        let multi_root = self.workspace().is_some_and(|workspace| workspace.folders.len() > 1);
        let description = multi_root.then(|| task.workspace_folder.as_deref().and_then(Path::file_name).map(|name| name.to_string_lossy().into_owned())).flatten();
        let detail = (self.setting("task.quickOpen.detail") != false).then(|| task.detail.clone()).flatten();
        Item { label: task.label.clone(), description, detail, buttons: vec![configure], ..Item::default() }
    }

    pub fn task_pick_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::Tasks(picker)) = self.quick_inputs.owner(id) else { return };
        let picker = picker.clone();
        match event {
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                let pick = picker.picks[handle as usize].clone();
                if let (PickAction::ConfigureDefault(group), TaskPick::Task(task)) = (picker.action, &pick) {
                    let all: Vec<Task> = picker.picks.iter().filter_map(|pick| if let TaskPick::Task(task) = pick { Some(task.clone()) } else { None }).collect();
                    self.hide_quick_input_by_id(id, window, cx);
                    return self.set_default_task(group, task.clone(), all, window, cx);
                }
                self.accept_task_pick(id, pick, picker.action, window, cx);
            }
            Event::TriggerItemButton(handle, button, _) => {
                let TaskPick::Task(task) = picker.picks[handle as usize].clone() else { return };
                if button == 1 {
                    self.remove_recent_task(&task);
                    let Some(input) = self.quick_inputs.get_mut(id) else { return };
                    input.remove_item(handle);
                    cx.notify();
                    return;
                }
                self.hide_quick_input_by_id(id, window, cx);
                self.configure_task(task, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    fn accept_task_pick(&mut self, id: i64, pick: TaskPick, action: PickAction, window: &mut Window, cx: &mut Context<Self>) {
        match pick {
            TaskPick::Type(task_type) => return self.show_second_level(id, Some(task_type), window, cx),
            TaskPick::ShowAll => return self.show_second_level(id, None, window, cx),
            TaskPick::GoBack => {
                let Some(input) = self.quick_inputs.get_mut(id) else { return };
                input.set_value(String::new());
                let configured = self.configured_tasks().into_iter().filter(|task| !task.hide).collect();
                let mut input = std::mem::replace(self.quick_inputs.get_mut(id).expect("checked above"), QuickInput::new(0, Kind::QuickPick));
                let picker = self.top_level_entries(&mut input, configured);
                *self.quick_inputs.get_mut(id).expect("checked above") = input;
                *self.quick_inputs.owner_mut(id).expect("checked above") = Owner::Tasks(picker);
                cx.notify();
                return;
            }
            _ => {}
        }
        self.hide_quick_input_by_id(id, window, cx);
        match (pick, action) {
            (TaskPick::Task(task), PickAction::Configure) => self.configure_task(task, window, cx),
            (TaskPick::OpenConfig(folder), _) => self.open_task_config(Some(folder), window, cx),
            (TaskPick::ConfigureDefault(group), _) => self.configure_default_task(group, window, cx),
            (TaskPick::Task(task), PickAction::Run) => self.run_picked_task(task, window, cx),
            (TaskPick::Task(task), PickAction::Terminate) => self.terminate_task(&task, window, cx),
            (TaskPick::Task(task), PickAction::Restart) => self.restart_task(&task, window, cx),
            (TaskPick::Instance(task, terminal), _) => self.restart_task_instance(&task, terminal, window, cx),
            (TaskPick::TerminateAll, _) => self.terminate_all_tasks(window, cx),
            (TaskPick::Configure, _) => self.run_configure_tasks(window, cx),
            (TaskPick::EnableDetection(task_type), _) => self.enable_task_detection(task_type, window, cx),
            _ => {}
        }
    }

    /// `doPickerSecondLevel`: the tasks of one type (or all of them), Go back, and the detection setting entries.
    fn show_second_level(&mut self, id: i64, task_type: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.quick_inputs.get_mut(id) else { return };
        input.busy = true;
        if task_type.is_some() {
            input.set_value(String::new());
        }
        cx.notify();
        let fetched = self.fetch_tasks(task_type.clone(), cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let tasks = fetched.await.unwrap_or_default();
            let _ = workbench.update_in(cx, |workbench, _, cx| {
                let mut tasks: Vec<Task> = tasks.into_iter().filter(|task| !task.hide && task_type.as_ref().is_none_or(|wanted| task.definition["type"] == *wanted)).collect();
                tasks.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
                let mut entries = Vec::new();
                let mut picks = Vec::new();
                for task in &tasks {
                    push_entry(&mut entries, &mut picks, workbench.task_item(task), TaskPick::Task(task.clone()));
                }
                let detection_types: Vec<String> = match &task_type {
                    Some(task_type) => {
                        if tasks.is_empty() {
                            push_entry(&mut entries, &mut picks, Item { label: format!("No {task_type} tasks found. Go back \u{21a9}"), always_show: true, ..Item::default() }, TaskPick::GoBack);
                        } else {
                            entries.push(Entry::Separator(None));
                            push_entry(&mut entries, &mut picks, Item { label: "Go back \u{21a9}".into(), always_show: true, ..Item::default() }, TaskPick::GoBack);
                        }
                        vec![task_type.clone()]
                    }
                    None => vec!["grunt".into(), "gulp".into(), "jake".into()],
                };
                for detection_type in detection_types {
                    if workbench.setting(&format!("{detection_type}.autoDetect")) == "off" {
                        let label = format!("$(gear) {} task detection is turned off. Enable {detection_type} task detection...", capitalize(&detection_type));
                        push_entry(&mut entries, &mut picks, Item { label, always_show: true, ..Item::default() }, TaskPick::EnableDetection(detection_type));
                    }
                }
                let Some(input) = workbench.quick_inputs.get_mut(id) else { return };
                input.set_items(entries);
                input.busy = false;
                let Some(owner) = workbench.quick_inputs.owner_mut(id) else { return };
                *owner = Owner::Tasks(TaskPicker { picks, action: PickAction::Run });
                cx.notify();
            });
        })
        .detach();
    }

    /// `handleSettingOption`: confirm, turn the detection on, and pick again among that type's tasks.
    fn enable_task_detection(&mut self, task_type: String, window: &mut Window, cx: &mut Context<Self>) {
        let message = format!("Task detection for {task_type} tasks causes files in any workspace you open to be run as code. Enabling {task_type} task detection is a user setting and will apply to any workspace you open. \n\n Do you want to enable {task_type} task detection for all workspaces?");
        let answer = dialog::show_message_box(window, PromptLevel::Warning, &message, None, &["Yes", "No"], None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            if !matches!(answer.await, Ok((0, _))) {
                return;
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.update_setting(&format!("{task_type}.autoDetect"), json!("on"));
                let mut input = QuickInput::new(0, Kind::QuickPick);
                input.placeholder = Some("Select the task to run".into());
                let id = workbench.quick_inputs.add(input, Owner::Tasks(TaskPicker { picks: Vec::new(), action: PickAction::Run }));
                workbench.show_quick_input(id, window, cx);
                workbench.show_second_level(id, Some(task_type), window, cx);
            });
        })
        .detach();
    }

    /// A picked task: a tasks.json configuration of a contributed type first gets the provided task it configures.
    fn run_picked_task(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        self.add_recent_task(&task);
        if !task.configures {
            drop(self.run_task(task, window, cx));
            return;
        }
        let fetched = self.fetch_tasks(task.definition["type"].as_str().map(str::to_owned), cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let tasks = fetched.await.unwrap_or_default();
            let resolved = tasks.into_iter().find(|candidate| candidate.definition["_key"] == task.definition["_key"]);
            let _ = workbench.update_in(cx, |workbench, window, cx| match resolved {
                Some(resolved) => drop(workbench.run_task(resolved, window, cx)),
                None => workbench.notify_error(format!("There is no task provider registered for tasks of type \"{}\".", task.definition["type"].as_str().unwrap_or_default())),
            });
        })
        .detach();
    }

    /// `_runTaskGroupCommand` for build or test: the single default task runs, else a picker of the group.
    fn run_group_command(&mut self, group: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let defaults: Vec<Task> = self.configured_tasks().into_iter().filter(|task| in_group(task, group) && task_is_default(task)).collect();
        if defaults.len() == 1 {
            self.run_picked_task(defaults[0].clone(), window, cx);
            return Outcome::done();
        }
        let fetched = self.fetch_tasks(None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let tasks: Vec<Task> = fetched.await.unwrap_or_default().into_iter().filter(|task| in_group(task, group)).collect();
            let defaults: Vec<Task> = tasks.iter().filter(|task| task_is_default(task)).cloned().collect();
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if defaults.len() == 1 {
                    workbench.run_picked_task(defaults[0].clone(), window, cx);
                    return;
                }
                let tasks = if defaults.is_empty() { tasks } else { defaults };
                let (select, not_found) = match group {
                    "build" => ("Select the build task to run", "No build task to run found. Configure Build Task..."),
                    _ => ("Select the test task to run", "No test task to run found. Configure Tasks..."),
                };
                workbench.show_task_list(tasks, select, (not_found, TaskPick::ConfigureDefault(group)), None, PickAction::Run, window, cx);
            });
        })
        .detach();
        Outcome::done()
    }

    /// `_showQuickPick`: the tasks sorted by label, the default entry when there is none, extra entries after.
    #[allow(clippy::too_many_arguments)]
    fn show_task_list(&mut self, mut tasks: Vec<Task>, placeholder: &str, default: (&str, TaskPick), extra: Option<(&str, TaskPick)>, action: PickAction, window: &mut Window, cx: &mut Context<Self>) {
        tasks.retain(|task| !task.hide);
        tasks.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        if tasks.is_empty() {
            push_entry(&mut entries, &mut picks, Item { label: default.0.into(), ..Item::default() }, default.1);
        }
        for task in tasks {
            let mut item = self.task_item(&task);
            if action != PickAction::Run {
                item.buttons.clear();
            }
            push_entry(&mut entries, &mut picks, item, TaskPick::Task(task));
        }
        if let Some((label, pick)) = extra.filter(|_| !picks.iter().all(|pick| matches!(pick, TaskPick::Nothing))) {
            entries.push(Entry::Separator(None));
            push_entry(&mut entries, &mut picks, Item { label: label.into(), ..Item::default() }, pick);
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(placeholder.into());
        input.match_on_description = true;
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Tasks(TaskPicker { picks, action }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `_handleInstancePolicy` prompt: the running instances of the task; the picked one restarts.
    pub(super) fn show_instance_picker(&mut self, task: &Task, terminals: Vec<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        for terminal in terminals {
            let mut item = self.task_item(task);
            item.buttons.clear();
            push_entry(&mut entries, &mut picks, item, TaskPick::Instance(task.clone(), terminal));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select an instance to terminate".into());
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Tasks(TaskPicker { picks, action: PickAction::Restart }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// `_runTerminateCommand`: `terminateAll`, a task identifier, or a picker of the running tasks.
    fn terminate_task_command(&mut self, argument: Value, window: &mut Window, cx: &mut Context<Self>) {
        if argument == "terminateAll" {
            return self.terminate_all_tasks(window, cx);
        }
        if let Some(task) = self.active_task_matching(&argument) {
            return self.terminate_task(&task, window, cx);
        }
        self.terminate_or_show(PickAction::Terminate, "Select a task to terminate", window, cx);
    }

    /// `_runRestartTaskCommand`: the only running task, a matching one, or a picker of the running tasks.
    fn restart_task_command(&mut self, argument: Value, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.running_tasks();
        if running.len() == 1 {
            return self.restart_task(&running[0], window, cx);
        }
        if let Some(task) = self.active_task_matching(&argument) {
            return self.restart_task(&task, window, cx);
        }
        self.terminate_or_show(PickAction::Restart, "Select the task to restart", window, cx);
    }

    fn terminate_or_show(&mut self, action: PickAction, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.running_tasks();
        match action {
            PickAction::Terminate => self.show_task_list(running, placeholder, ("No task is currently running", TaskPick::Nothing), Some(("All Running Tasks", TaskPick::TerminateAll)), action, window, cx),
            _ if placeholder.starts_with("Select the task to show") => {
                // `_showTasks`: one running task shows its terminal, more get a picker.
                match running.as_slice() {
                    [] => drop(self.notify(Notification::new(Severity::Info, "There are no task results to show"))),
                    [only] => self.reveal_task_terminal(only, window, cx),
                    _ => self.show_task_list(running, placeholder, ("No task is currently running", TaskPick::Nothing), None, PickAction::Run, window, cx),
                }
            }
            _ => self.show_task_list(running, placeholder, ("No task to restart", TaskPick::Nothing), None, action, window, cx),
        }
    }

    fn active_task_matching(&self, argument: &Value) -> Option<Task> {
        match argument {
            Value::Null => None,
            reference => {
                let running = self.running_tasks();
                running.iter().find(|task| match reference {
                    Value::String(label) => task.label == *label || task.name == *label,
                    Value::Object(identifier) => identifier.iter().all(|(key, value)| key == "_key" || task.definition.get(key) == Some(value)),
                    _ => false,
                }).cloned()
            }
        }
    }

    fn terminate_task(&mut self, task: &Task, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self.task_terminal(task) {
            self.kill_terminal(terminal, window, cx);
        }
    }

    fn terminate_all_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for task in self.running_tasks() {
            self.terminate_task(&task, window, cx);
        }
    }

    /// `_restart`: terminate, then run the task again.
    fn restart_task(&mut self, task: &Task, window: &mut Window, cx: &mut Context<Self>) {
        self.terminate_task(task, window, cx);
        drop(self.run_task(task.clone(), window, cx));
    }

    fn reveal_task_terminal(&mut self, task: &Task, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self.task_terminal(task) {
            self.terminal_view.set_active(terminal);
            self.show_terminal_tab(window, cx);
        }
    }

    /// `_openTaskFile`: an existing tasks.json opens; without one, a template is picked and written.
    pub(super) fn open_task_config(&mut self, folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = folder else {
            self.notify(Notification::new(Severity::Info, "Tasks are only available on a workspace folder."));
            return;
        };
        let existing = [".vscode", ".vsrustium"].iter().map(|dir| folder.join(dir).join("tasks.json")).find(|path| path.exists());
        if let Some(path) = existing {
            return self.open_file(path, true, window, cx);
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a Task Template".into());
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        for template in crate::tasks::templates::templates() {
            let item = Item { label: template.label.into(), detail: Some(template.description.into()), ..Item::default() };
            push_entry(&mut entries, &mut picks, item, TaskPick::Nothing);
        }
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::TaskTemplates(folder));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn task_template_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(Owner::TaskTemplates(folder)) = self.quick_inputs.owner(id) else { return };
                let folder = folder.clone();
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                self.hide_quick_input_by_id(id, window, cx);
                let template = &crate::tasks::templates::templates()[handle as usize];
                let path = folder.join(".vscode").join("tasks.json");
                let written = std::fs::create_dir_all(folder.join(".vscode")).and_then(|()| std::fs::write(&path, template.content));
                match written {
                    Ok(()) => self.open_file(path, true, window, cx),
                    Err(error) => self.notify_error(format!("Unable to write {}: {error}", path.display())),
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `_getRecentTasks`: the stored entries still configured or of a contributed type, most recent first.
    fn recent_tasks(&self, configured: &[Task]) -> Vec<Task> {
        let mut recent = Vec::new();
        for entry in self.stored_recent_tasks() {
            let key = entry["key"].as_str().unwrap_or_default();
            if let Some(task) = configured.iter().find(|task| recent_key(task) == key) {
                recent.push(task.clone());
                continue;
            }
            let custom = entry["definition"]["type"] == CUSTOMIZED_TASK_TYPE;
            if custom || entry["label"].is_null() {
                continue;
            }
            let label = entry["label"].as_str().unwrap_or_default().to_owned();
            recent.push(Task {
                id: key.to_owned(),
                name: label.clone(),
                label,
                definition: entry["definition"].clone(),
                workspace_folder: entry["folder"].as_str().map(PathBuf::from),
                configures: true,
                ..Task::default()
            });
        }
        recent
    }

    fn stored_recent_tasks(&self) -> Vec<Value> {
        let storage = self.workspace_storage.lock().unwrap();
        let stored = storage.get(RECENT_TASKS_KEY).and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok());
        stored.unwrap_or_default()
    }

    fn store_recent_tasks(&self, entries: Vec<Value>) {
        self.workspace_storage.lock().unwrap().set(RECENT_TASKS_KEY, Value::Array(entries).to_string());
    }

    /// `_setRecentlyUsedTask`: most recent first, at most `task.quickOpen.history` entries.
    pub(super) fn add_recent_task(&mut self, task: &Task) {
        self.tasks.last = Some(task.clone());
        let limit = self.setting("task.quickOpen.history").as_u64().unwrap_or(30) as usize;
        let key = recent_key(task);
        let mut entries = self.stored_recent_tasks();
        entries.retain(|entry| entry["key"] != key.as_str());
        entries.insert(0, json!({ "key": key, "label": task.label, "definition": task.definition, "folder": task.workspace_folder }));
        entries.truncate(limit);
        self.store_recent_tasks(entries);
    }

    fn remove_recent_task(&mut self, task: &Task) {
        let key = recent_key(task);
        let mut entries = self.stored_recent_tasks();
        entries.retain(|entry| entry["key"] != key.as_str());
        self.store_recent_tasks(entries);
    }
}

pub(super) fn push_entry(entries: &mut Vec<Entry>, picks: &mut Vec<TaskPick>, item: Item, pick: TaskPick) {
    entries.push(Entry::Item(Item { handle: picks.len() as i64, ..item }));
    picks.push(pick);
}

/// `Task.getKey`: the folder and label of a tasks.json task, the definition key of a contributed one.
fn recent_key(task: &Task) -> String {
    let folder = task.workspace_folder.as_deref().map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default();
    match task.definition["type"] == CUSTOMIZED_TASK_TYPE {
        true => format!("{folder}|{}", task.label),
        false => format!("{folder}|{}", task.definition["_key"].as_str().unwrap_or_default()),
    }
}

fn in_group(task: &Task, group: &str) -> bool {
    task.group.as_ref().is_some_and(|value| value["_id"] == group)
}

fn task_is_default(task: &Task) -> bool {
    task.group.as_ref().is_some_and(|value| value["isDefault"] == true)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_key_separates_custom_and_contributed_tasks() {
        let custom = Task { label: "echo".into(), definition: json!({ "type": CUSTOMIZED_TASK_TYPE, "_key": "$customized,echo" }), workspace_folder: Some("/w".into()), ..Task::default() };
        let npm = Task { label: "npm: build".into(), definition: json!({ "type": "npm", "_key": "npm,build," }), workspace_folder: Some("/w".into()), ..Task::default() };
        assert_eq!(recent_key(&custom), "/w|echo");
        assert_eq!(recent_key(&npm), "/w|npm,build,");
        assert_eq!(capitalize("grunt"), "Grunt");
    }

    #[test]
    fn group_defaults() {
        let task = Task { group: Some(json!({ "_id": "build", "isDefault": true })), ..Task::default() };
        assert!(in_group(&task, "build") && task_is_default(&task));
        assert!(!in_group(&task, "test"));
    }
}
