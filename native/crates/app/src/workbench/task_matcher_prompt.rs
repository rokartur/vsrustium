//! abstractTaskService.ts `_shouldAttachProblemMatcher` and `_attachProblemMatcher`: a picked contributed or custom
//! task without problem matchers asks which output to scan before it runs.

use gpui::{Context, Window};
use serde_json::{Map, Value, json};

use super::Workbench;
use super::opener::{OpenOptions, Target};
use super::task_quick_pick::{PickAction, TaskPick, TaskPicker, push_entry};
use crate::command_palette::locale_compare;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Entry, Item, Kind, QuickInput};
use crate::tasks::configuration::{CUSTOMIZED_TASK_TYPE, Task};
use crate::uri::Uri;

const NEVER_PROMPT: &str = "task.problemMatchers.neverPrompt";

#[derive(Clone)]
pub enum MatcherChoice {
    Continue,
    Never,
    NeverForType(String),
    LearnMore,
    Matcher { name: String, watching: bool },
}

impl Workbench {
    /// `run(task, { attachProblemMatcher: true })`.
    pub(super) fn run_attaching_problem_matcher(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        if !self.should_attach_problem_matcher(&task) {
            drop(self.run_task(task, window, cx));
            return;
        }
        let task_type = matcher_task_type(&task);
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        let choices = [
            ("Continue without scanning the task output".to_owned(), MatcherChoice::Continue),
            ("Never scan the task output for this task".to_owned(), MatcherChoice::Never),
            (format!("Never scan the task output for {task_type} tasks"), MatcherChoice::NeverForType(task_type.clone())),
            ("Learn more about scanning the task output".to_owned(), MatcherChoice::LearnMore),
        ];
        for (label, choice) in choices {
            push_entry(&mut entries, &mut picks, Item { label, ..Item::default() }, TaskPick::Matcher(task.clone(), choice));
        }
        entries.push(Entry::Separator(Some("associate".into())));
        let mut matchers: Vec<_> = self.tasks.matchers.pickable().collect();
        matchers.sort_by(|a, b| locale_compare(a.label.as_deref().unwrap_or_default(), b.label.as_deref().unwrap_or_default()));
        for matcher in matchers {
            let name = matcher.name.clone().expect("registered matchers are named");
            let label = matcher.label.clone().unwrap_or_else(|| name.clone());
            let description = (label != name).then(|| format!("${name}"));
            let choice = MatcherChoice::Matcher { name, watching: matcher.watching.is_some() };
            push_entry(&mut entries, &mut picks, Item { label, description, ..Item::default() }, TaskPick::Matcher(task.clone(), choice));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select for which kind of errors and warnings to scan the task output".into());
        input.match_on_description = true;
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Tasks(TaskPicker { picks, action: PickAction::AttachMatcher }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    /// The picked entry; a dismissed picker runs the task as it is.
    pub(super) fn accept_problem_matcher(&mut self, task: Task, choice: MatcherChoice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            MatcherChoice::Continue => {}
            MatcherChoice::LearnMore => {
                let uri = Uri::parse("https://code.visualstudio.com/docs/editor/tasks#_defining-a-problem-matcher").expect("a valid URL");
                drop(self.open_link(Target::Uri(uri), OpenOptions::default(), window, cx));
                return;
            }
            MatcherChoice::Never => self.customize_task(&task, Map::from_iter([("problemMatcher".into(), json!([]))]), true, window, cx),
            MatcherChoice::Matcher { name, watching } => {
                let reference = json!(format!("${name}"));
                let mut properties = Map::from_iter([("problemMatcher".into(), json!([reference.clone()]))]);
                if watching {
                    properties.insert("isBackground".into(), json!(true));
                }
                self.customize_task(&task, properties, true, window, cx);
                let task = Task { problem_matchers: vec![reference], is_background: task.is_background || watching, ..task };
                drop(self.run_task(task, window, cx));
                return;
            }
            MatcherChoice::NeverForType(task_type) => {
                let mut never = match self.setting(NEVER_PROMPT) {
                    Value::Bool(true) => return drop(self.run_task(task, window, cx)),
                    Value::Object(never) => never,
                    _ => Map::new(),
                };
                never.insert(task_type, json!(true));
                self.update_setting(NEVER_PROMPT, Value::Object(never));
            }
        }
        drop(self.run_task(task, window, cx));
    }

    fn should_attach_problem_matcher(&self, task: &Task) -> bool {
        let enabled = match self.setting(NEVER_PROMPT) {
            Value::Bool(never) => !never,
            never => never[matcher_task_type(task)] != true,
        };
        // `_canCustomize`: a contributed task needs its workspace folder.
        let can_customize = task.definition["type"] == CUSTOMIZED_TASK_TYPE || task.workspace_folder.is_some();
        let build_or_none = task.group.as_ref().is_none_or(|group| group["_id"] == "build" && group["isDefault"] != true);
        enabled && can_customize && build_or_none && task.problem_matchers.is_empty() && !task.has_defined_matchers
    }
}

/// `_getTypeForTask`: a custom task's `type` (shell or process), else the definition's.
fn matcher_task_type(task: &Task) -> String {
    if task.definition["type"] != CUSTOMIZED_TASK_TYPE {
        return task.definition["type"].as_str().unwrap_or_default().to_owned();
    }
    let process = task.execution.as_ref().is_some_and(|execution| execution.get("process").is_some());
    if process { "process" } else { "shell" }.to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[gpui::test]
    fn a_contributed_task_without_matchers_asks_for_one(cx: &mut gpui::TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        let gulp = Task { label: "gulp: hello".into(), definition: json!({ "type": "gulp", "task": "hello" }), workspace_folder: Some("/w".into()), ..Task::default() };
        workbench.update_in(cx, |workbench, window, cx| {
            workbench.tasks.matchers = crate::tasks::problem_matcher::Registry::new(&[]);
            let with_matcher = Task { problem_matchers: vec![json!("$go")], ..gulp.clone() };
            assert!(!workbench.should_attach_problem_matcher(&with_matcher));
            assert!(!workbench.should_attach_problem_matcher(&Task { group: Some(json!({ "_id": "test" })), ..gulp.clone() }));
            workbench.run_attaching_problem_matcher(gulp, window, cx);
            let id = workbench.quick_inputs.find_id(|owner| matches!(owner, Owner::Tasks(picker) if picker.action == PickAction::AttachMatcher)).unwrap();
            let labels: Vec<String> = workbench.quick_inputs.get(id).unwrap().items().iter().filter_map(|entry| if let Entry::Item(item) = entry { Some(item.label.clone()) } else { None }).collect();
            assert_eq!(labels[2], "Never scan the task output for gulp tasks");
            assert_eq!(labels[4], "ESLint compact problems");
            assert!(!labels.contains(&"Less problems".to_owned()));
        });
    }

    /// `_runTaskCommand`: a task name that matches runs with `attachProblemMatcher`, so the prompt comes first.
    #[gpui::test]
    fn run_task_with_a_name_asks_for_a_problem_matcher(cx: &mut gpui::TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let folder = std::env::temp_dir().join(format!("code-gpui-run-task-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let workspace = config::workspace::Workspace::folder(folder.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(workspace), crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(sender), cx));
        workbench.update_in(cx, |workbench, window, cx| {
            workbench.tasks.matchers = crate::tasks::problem_matcher::Registry::new(&[]);
            workbench.tasks.providers.push((1, "gulp".into()));
            drop(workbench.run_task_command(json!("gulp: hello"), window, cx));
        });
        cx.run_until_parked();
        let dto = json!({ "name": "hello", "definition": { "type": "gulp", "task": "hello" }, "source": { "label": "gulp", "extensionId": "vscode.gulp" }, "execution": { "commandLine": "gulp hello" }, "problemMatchers": [] });
        // The activations answer first (a dropped reply), then the provider.
        for _ in 0..2 {
            for message in receiver.try_iter().filter(|message| message.method == "$provideTasks") {
                let _ = message.reply.unwrap().send(Ok(json!({ "tasks": [dto.clone()] })));
            }
            cx.run_until_parked();
        }
        let asks = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.find_id(|owner| matches!(owner, Owner::Tasks(picker) if picker.action == PickAction::AttachMatcher)).is_some());
        assert!(asks);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
