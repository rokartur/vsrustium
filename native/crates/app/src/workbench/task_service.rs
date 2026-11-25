//! mainThreadTask.ts with abstractTaskService.ts and terminalTaskSystem.ts: tasks from tasks.json and from the
//! extensions' providers, run in feature terminals (shell and process) or extension pseudoterminals (custom
//! executions), their dependencies, and problem matchers feeding the markers.

use std::collections::HashMap;
use std::path::PathBuf;

use exthost::session::ForwardedRequest;
use futures::channel::oneshot;
use futures::future::join_all;
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::tasks::configuration::{Task, parse_tasks_json};
use crate::tasks::definitions::{TaskDefinition, definitions};
use crate::tasks::problem_collector::{MarkerChange, Markers, ProblemCollector};
use crate::tasks::problem_matcher::Registry;
use crate::terminal::ShellLaunchConfig;

#[derive(Default)]
pub struct TaskService {
    /// `$registerTaskProvider`: handle and task type.
    pub(super) providers: Vec<(i64, String)>,
    definitions: Vec<TaskDefinition>,
    matchers: Registry,
    /// `UUIDMap`: custom task ids by folder and label, stable across reads.
    uuids: HashMap<(PathBuf, String), String>,
    /// Tasks the host referred to by id (`$executeTask` with a DTO), for `$getTaskExecution`.
    known: HashMap<String, Task>,
    active: Vec<ActiveTask>,
    /// The last task a user ran, for `reRunTask`.
    pub(super) last: Option<Task>,
}

struct ActiveTask {
    task: Task,
    terminal: u32,
    /// `startId` of a custom execution (`reserveCustomExecution`).
    start_id: Option<String>,
    collector: Option<ProblemCollector>,
    /// Who waits for the task's end (`run` and the tasks depending on it), with its exit code.
    waiters: Vec<oneshot::Sender<Option<i32>>>,
}

/// What `$fetchTasks` and the run commands search: every configured and provided task.
type Fetched = oneshot::Receiver<Vec<Task>>;

impl TaskService {
    pub fn new(contributions: &[(&str, &Value)]) -> Self {
        let contributes: Vec<&Value> = contributions.iter().map(|(_, contributes)| *contributes).collect();
        TaskService { definitions: definitions(contributions), matchers: Registry::new(&contributes), ..Default::default() }
    }

    fn definition(&self, task_type: &str) -> Option<&TaskDefinition> {
        self.definitions.iter().find(|definition| definition.task_type == task_type)
    }
}

/// The markers service as the collector sees it.
struct WorkbenchMarkers<'a>(&'a Workbench);

impl Markers for WorkbenchMarkers<'_> {
    fn resources_of(&self, owner: &str) -> Vec<PathBuf> {
        self.0.marker_resources_of(owner)
    }

    fn is_open(&self, resource: &PathBuf) -> bool {
        self.0.editors.editors.iter().any(|editor| editor.document.path == *resource)
    }
}

impl Workbench {
    /// The task types, problem patterns and matchers of the running extensions; the run commands.
    pub fn start_tasks(&mut self) {
        let contributions: Vec<(&str, &Value)> = self.extensions.iter().map(|extension| (extension.id.as_str(), &extension.description["contributes"])).collect();
        self.tasks = TaskService::new(&contributions);
        self.commands.register("workbench.action.tasks.runTask", |workbench, args, window, cx| workbench.run_task_command(args.into_iter().next().unwrap_or(Value::Null), window, cx));
        self.register_task_pick_actions();
    }

    pub(super) fn running_tasks(&self) -> Vec<Task> {
        self.tasks.active.iter().map(|active| active.task.clone()).collect()
    }

    pub(super) fn task_terminal(&self, task: &Task) -> Option<u32> {
        self.tasks.active.iter().find(|active| active.task.id == task.id).map(|active| active.terminal)
    }

    pub fn handle_task_request(&mut self, request: ForwardedRequest, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| request.args.get(index).cloned().unwrap_or(Value::Null);
        match request.method.as_str() {
            "$registerTaskProvider" => {
                self.tasks.providers.push((arg(0).as_i64().unwrap_or_default(), arg(1).as_str().unwrap_or_default().to_owned()));
                Outcome::done()
            }
            "$unregisterTaskProvider" => {
                let handle = arg(0).as_i64();
                self.tasks.providers.retain(|(known, _)| Some(*known) != handle);
                Outcome::done()
            }
            "$createTaskId" => match self.task_from_dto(&arg(0)) {
                Some(task) => Outcome::Ready(Ok(json!(task.id))),
                None => Outcome::error("Task could not be created from DTO"),
            },
            "$fetchTasks" => {
                let filter_type = arg(0)["type"].as_str().map(str::to_owned);
                let fetched = self.fetch_tasks(filter_type.clone(), cx);
                let (reply, receiver) = oneshot::channel();
                cx.background_executor()
                    .spawn(async move {
                        let tasks = fetched.await.unwrap_or_default();
                        let dtos: Vec<Value> = tasks.iter().filter(|task| filter_type.as_ref().is_none_or(|kind| task.definition["type"] == *kind)).map(Task::to_dto).collect();
                        let _ = reply.send(Ok(json!(dtos)));
                    })
                    .detach();
                Outcome::Pending(receiver)
            }
            "$getTaskExecution" | "$executeTask" => {
                let value = arg(0);
                let execute = request.method == "$executeTask";
                // `TaskHandleDTO`: an id and a workspace folder; the task is looked up again.
                if value["id"].is_string() && !value["workspaceFolder"].is_null() {
                    let id = value["id"].as_str().unwrap_or_default().to_owned();
                    let task_type = value["type"].as_str().map(str::to_owned);
                    let fetched = self.fetch_tasks(task_type, cx);
                    let (reply, receiver) = oneshot::channel();
                    cx.spawn_in(window, async move |workbench, cx| {
                        let tasks = fetched.await.unwrap_or_default();
                        let Some(task) = tasks.into_iter().find(|task| task.id == id) else {
                            let _ = reply.send(Err(crate::commands::error_value("Task not found")));
                            return;
                        };
                        let _ = reply.send(Ok(json!({ "id": id, "task": task.to_dto() })));
                        if execute {
                            let _ = workbench.update_in(cx, |workbench, window, cx| drop(workbench.run_task(task, window, cx)));
                        }
                    })
                    .detach();
                    return Outcome::Pending(receiver);
                }
                let Some(task) = self.task_from_dto(&value) else { return Outcome::error("Task could not be created from DTO") };
                let result = json!({ "id": task.id, "task": task.to_dto() });
                self.tasks.known.insert(task.id.clone(), task.clone());
                if execute {
                    drop(self.run_task(task, window, cx));
                }
                Outcome::Ready(Ok(result))
            }
            "$customExecutionComplete" => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                let Some(index) = self.tasks.active.iter().position(|active| active.task.id == id) else { return Outcome::error("Task to mark as complete not found") };
                let code = arg(1).as_i64().map(|code| code as i32);
                self.task_ended(index, code, window, cx);
                Outcome::done()
            }
            "$terminateTask" => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                let Some(terminal) = self.tasks.active.iter().find(|active| active.task.id == id).map(|active| active.terminal) else {
                    return Outcome::error("Task to terminate not found");
                };
                self.kill_terminal(terminal, window, cx);
                Outcome::done()
            }
            // `$registerSupportedExecutions` and `$registerTaskSystem`: the port runs every execution kind locally.
            _ => Outcome::done(),
        }
    }

    fn task_from_dto(&self, dto: &Value) -> Option<Task> {
        let task_type = dto["definition"]["type"].as_str().unwrap_or_default();
        let folder = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        Task::from_dto(dto, self.tasks.definition(task_type), folder.as_deref())
    }

    /// `getWorkspaceTasks` then the providers (`_getGroupedTasks`): tasks.json of each folder, the provided tasks
    /// of `task_type` (every type without one), and the configurations merged into the tasks they configure.
    pub(super) fn fetch_tasks(&mut self, task_type: Option<String>, cx: &mut Context<Self>) -> Fetched {
        let configured = self.configured_tasks();
        // `_activateTaskProviders`: the run command's event and `onTaskType:` of the type, or of every type.
        let mut events = vec!["onCommand:workbench.action.tasks.runTask".to_owned()];
        match &task_type {
            Some(task_type) => events.push(format!("onTaskType:{task_type}")),
            None => events.extend(self.tasks.definitions.iter().map(|definition| format!("onTaskType:{}", definition.task_type))),
        }
        let activations: Vec<_> = events.iter().map(|event| self.host.activate_by_event(event)).collect();
        let (reply, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let timeout = cx.background_executor().timer(std::time::Duration::from_secs(5));
            futures::future::select(join_all(activations), timeout).await;
            let Ok(requests) = workbench.update(cx, |workbench, _| workbench.provide_tasks(task_type.as_deref())) else { return };
            let provided = requests.await;
            let _ = reply.send(merge_tasks(configured, provided));
        })
        .detach();
        receiver
    }

    /// `$provideTasks` of the providers registered for `task_type` (all without one).
    fn provide_tasks(&self, task_type: Option<&str>) -> impl std::future::Future<Output = Vec<Task>> + use<> {
        let mut requests = Vec::new();
        for (handle, provider_type) in &self.tasks.providers {
            if task_type.is_some_and(|wanted| wanted != provider_type) {
                continue;
            }
            let valid_types: serde_json::Map<String, Value> = self.tasks.definitions.iter().map(|definition| (definition.task_type.clone(), json!(true))).chain([("shell".into(), json!(true)), ("process".into(), json!(true))]).collect();
            requests.push(self.host.call("ExtHostTask", "$provideTasks", vec![json!(handle), Value::Object(valid_types)]));
        }
        let definitions = self.tasks.definitions.clone();
        let folder = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        async move {
            let mut provided = Vec::new();
            for set in join_all(requests).await.into_iter().flatten().flatten() {
                for dto in set["tasks"].as_array().into_iter().flatten() {
                    let definition = definitions.iter().find(|definition| definition.task_type == dto["definition"]["type"].as_str().unwrap_or_default());
                    if let Some(task) = Task::from_dto(dto, definition, folder.as_deref()) {
                        provided.push(task);
                    }
                }
            }
            provided
        }
    }


    /// tasks.json of each folder through the configuration service (`.vscode/` and `.vsrustium/`).
    pub(super) fn configured_tasks(&mut self) -> Vec<Task> {
        let folders: Vec<PathBuf> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).collect()).unwrap_or_default();
        let mut tasks = Vec::new();
        for folder in folders {
            let inspected = self.settings.lock().unwrap().configuration.inspect("tasks", &config::configuration::Overrides { resource: Some(&folder), override_identifier: None });
            let Some(config) = inspected.workspace_folder.value.or(inspected.workspace.value) else { continue };
            let folder_uri = json!({ "$mid": 1, "scheme": "file", "path": folder });
            let uuids = &mut self.tasks.uuids;
            let (parsed, errors) = parse_tasks_json(&config, &folder, &folder_uri, &self.tasks.definitions, &mut |label| uuids.entry((folder.clone(), label.to_owned())).or_insert_with(exthost::main_thread_storage::generate_uuid).clone());
            for error in errors {
                self.log_task_error(&error);
            }
            tasks.extend(parsed);
        }
        tasks
    }

    /// `_outputChannel.append`: configuration errors go to the Tasks output channel.
    // shortcut: the Tasks output channel is not registered; the errors go to the Window log.
    fn log_task_error(&self, message: &str) {
        eprintln!("[tasks] {message}");
    }

    /// `run`: dependencies first, then the task; the receiver gets its exit code once it ends.
    pub(super) fn run_task(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Option<i32>> {
        let (reply, receiver) = oneshot::channel();
        if task.depends_on.is_empty() {
            self.start_task(task, reply, window, cx);
            return receiver;
        }
        let fetched = self.fetch_tasks(None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let all = fetched.await.unwrap_or_default();
            let dependencies: Vec<Task> = task.depends_on.iter().filter_map(|reference| find_task(&all, reference)).collect();
            let ok = run_dependencies(&workbench, cx, dependencies, task.depends_in_sequence).await;
            if !ok {
                // `executeTask` resolves with no exit code when a dependency failed.
                let _ = reply.send(None);
                return;
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.start_task(task, reply, window, cx));
        })
        .detach();
        receiver
    }

    /// terminalTaskSystem.ts `_executeInTerminal`: a terminal named after the task runs it.
    fn start_task(&mut self, task: Task, reply: oneshot::Sender<Option<i32>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(execution) = task.execution.clone() else {
            // A task with only dependencies ends as soon as they did.
            let _ = reply.send(Some(0));
            return;
        };
        let custom = execution["customExecution"] == "customExecution";
        let (launch, echo) = match custom {
            true => (ShellLaunchConfig { name: Some(task.name.clone()), extension_pty: true, is_feature_terminal: true, cwd: self.terminal_initial_cwd(), ..Default::default() }, task.label.clone()),
            false => self.task_launch_config(&task, &execution),
        };
        let terminal = self.add_terminal(launch, None, window, cx);
        // `initialText`: `formatMessageForTerminal` of 'Executing task: {0}' unless the presentation does not echo.
        if task.presentation["echo"] != false {
            let colors = crate::terminal::colors::TerminalColors::new(&self.theme);
            if let Some(instance) = self.terminal_view.instance_mut(terminal) {
                let message = format!("\x1b[0m\x1b[7m * \x1b[0m Executing task: {echo} \x1b[0m\n\r");
                let _ = instance.process(message.as_bytes(), &colors);
            }
        }
        self.show_terminal_tab(window, cx);
        let execution_dto = json!({ "id": task.id, "task": task.to_dto() });
        let start_id = custom.then(exthost::main_thread_storage::generate_uuid);
        if let Some(start_id) = &start_id {
            self.host.send("ExtHostTask", "$onWillStartTask", vec![json!(task.id), json!(start_id)]);
        }
        self.host.send("ExtHostTask", "$onDidStartTask", vec![execution_dto.clone(), json!(terminal), task.definition.clone(), json!(start_id)]);
        let pid = self.terminal_view.instance(terminal).and_then(|instance| instance.pid);
        if let Some(pid) = pid {
            self.host.send("ExtHostTask", "$onDidStartTaskProcess", vec![json!({ "id": task.id, "processId": pid })]);
        }
        let mut collector = self.task_collector(&task);
        if let Some(collector) = &mut collector {
            // `aboutToStart`: background matchers active on start begin with the task.
            collector.about_to_start(&WorkbenchMarkers(self));
        }
        if collector.is_some() {
            self.host.send("ExtHostTask", "$onDidStartTaskProblemMatchers", vec![json!({ "execution": execution_dto })]);
        }
        self.tasks.active.push(ActiveTask { task, terminal, start_id, collector, waiters: vec![reply] });
    }

    /// `_createShellLaunchConfig` for macOS: the automation shell (`-c` and the command line) or the process.
    fn task_launch_config(&self, task: &Task, execution: &Value) -> (ShellLaunchConfig, String) {
        let options = &execution["options"];
        let cwd = options["cwd"].as_str().map(PathBuf::from).or_else(|| task.workspace_folder.clone()).unwrap_or_else(|| self.terminal_initial_cwd());
        let env: HashMap<String, Option<String>> = options["env"].as_object().into_iter().flatten().map(|(key, value)| (key.clone(), value.as_str().map(str::to_owned))).collect();
        let (executable, args, echo) = match execution["process"].as_str() {
            Some(process) => {
                let args: Vec<String> = execution["args"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str().map(str::to_owned)).collect();
                let echo = std::iter::once(process.to_owned()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ");
                (PathBuf::from(process), args, echo)
            }
            None => {
                let command_line = shell_command_line(execution);
                let (executable, mut args) = match options["executable"].as_str() {
                    Some(shell) => (PathBuf::from(shell), options["shellArgs"].as_array().into_iter().flatten().filter_map(|arg| arg.as_str().map(str::to_owned)).collect()),
                    None => {
                        let profile = self.default_terminal_profile();
                        let mut args = profile.args;
                        args.push("-c".into());
                        (profile.path, args)
                    }
                };
                args.push(command_line.clone());
                (executable, args, command_line)
            }
        };
        let launch = ShellLaunchConfig {
            name: Some(task.name.clone()),
            executable,
            args,
            cwd,
            env: self.terminal_environment(&env),
            is_feature_terminal: true,
            ..Default::default()
        };
        (launch, echo)
    }

    /// The task's problem matchers by name or inline, `None` without any.
    fn task_collector(&self, task: &Task) -> Option<ProblemCollector> {
        let folder = task.workspace_folder.as_ref().map(|folder| folder.to_string_lossy().into_owned());
        let mut matchers = Vec::new();
        for reference in &task.problem_matchers {
            let matcher = match reference {
                Value::String(name) => self.tasks.matchers.get(name.trim_start_matches('$')).cloned(),
                description => self.tasks.matchers.parse(description),
            };
            let Some(mut matcher) = matcher else {
                self.log_task_error(&format!("Problem matcher {reference} can't be resolved. The matcher will be ignored"));
                continue;
            };
            if let (Some(prefix), Some(folder)) = (&matcher.file_prefix, &folder) {
                matcher.file_prefix = Some(prefix.replace("${workspaceFolder}", folder));
            }
            matchers.push(matcher);
        }
        (!matchers.is_empty()).then(|| ProblemCollector::new(matchers, &WorkbenchMarkers(self)))
    }

    /// Task terminal output for its problem collector.
    pub(super) fn task_output(&mut self, terminal: u32, text: &str, cx: &mut Context<Self>) {
        let Some(index) = self.tasks.active.iter().position(|active| active.terminal == terminal) else { return };
        let Some(mut collector) = self.tasks.active[index].collector.take() else { return };
        let mut changes = Vec::new();
        collector.process_output(text, &WorkbenchMarkers(self), &mut changes);
        self.tasks.active[index].collector = Some(collector);
        self.apply_marker_changes(changes, cx);
    }

    /// A task terminal's process exited; returns whether the terminal belongs to a task (and stays open).
    pub(super) fn task_process_exited(&mut self, terminal: u32, code: Option<i32>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.tasks.active.iter().position(|active| active.terminal == terminal) else { return false };
        let active = &self.tasks.active[index];
        if active.start_id.is_none() {
            self.host.send("ExtHostTask", "$onDidEndTaskProcess", vec![json!({ "id": active.task.id, "exitCode": code })]);
        }
        self.task_ended(index, code, window, cx);
        true
    }

    /// The task's end: problem matchers finish, the host hears `$OnDidEndTask`, the waiters get the exit code.
    fn task_ended(&mut self, index: usize, code: Option<i32>, _window: &mut Window, cx: &mut Context<Self>) {
        let mut active = self.tasks.active.remove(index);
        let execution = json!({ "id": active.task.id, "task": active.task.to_dto() });
        if let Some(mut collector) = active.collector.take() {
            let mut changes = Vec::new();
            collector.done(&WorkbenchMarkers(self), &mut changes);
            self.apply_marker_changes(changes, cx);
            self.host.send("ExtHostTask", "$onDidEndTaskProblemMatchers", vec![json!({ "execution": execution, "hasErrors": collector.max_severity == Some(crate::tasks::problem_matcher::SEVERITY_ERROR) })]);
        }
        self.host.send("ExtHostTask", "$OnDidEndTask", vec![execution, json!(active.start_id)]);
        for waiter in active.waiters {
            let _ = waiter.send(code);
        }
    }

    fn apply_marker_changes(&mut self, changes: Vec<MarkerChange>, cx: &mut Context<Self>) {
        for change in changes {
            match change {
                MarkerChange::Set { owner, resource, markers } => {
                    let uri = json!({ "$mid": 1, "scheme": "file", "path": resource });
                    self.change_many_markers(&owner, &json!([[uri, markers, false]]), cx);
                }
                MarkerChange::Remove { owner, resources } => {
                    let entries: Vec<Value> = resources.iter().map(|resource| json!([{ "$mid": 1, "scheme": "file", "path": resource }, [], false])).collect();
                    self.change_many_markers(&owner, &json!(entries), cx);
                }
            }
        }
    }

    /// `workbench.action.tasks.runTask`: a label, a task identifier object, or the pick of every task.
    pub(super) fn run_task_command(&mut self, argument: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if argument.is_null() {
            self.show_run_task_picker(None, window, cx);
            return Outcome::done();
        }
        let fetched = self.fetch_tasks(argument["type"].as_str().map(str::to_owned), cx);
        let (reply, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let tasks = fetched.await.unwrap_or_default();
            let Some(task) = find_task(&tasks, &argument) else {
                // `_doRunTaskCommand(tasks, type, name)`: the picker, filtered by the name.
                let name = argument.as_str().map(str::to_owned).or_else(|| argument["task"].as_str().map(str::to_owned));
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.show_run_task_picker(name, window, cx));
                let _ = reply.send(Ok(Value::Null));
                return;
            };
            let Ok(ended) = workbench.update_in(cx, |workbench, window, cx| {
                workbench.add_recent_task(&task);
                workbench.run_task(task, window, cx)
            }) else {
                return;
            };
            let code = ended.await.ok().flatten();
            let _ = reply.send(Ok(json!({ "exitCode": code })));
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

async fn run_dependencies(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, dependencies: Vec<Task>, in_sequence: bool) -> bool {
    if in_sequence {
        for task in dependencies {
            let Ok(ended) = workbench.update_in(cx, |workbench, window, cx| workbench.run_task(task, window, cx)) else { return false };
            if ended.await.ok().flatten() != Some(0) {
                return false;
            }
        }
        return true;
    }
    let mut started = Vec::new();
    for task in dependencies {
        let Ok(ended) = workbench.update_in(cx, |workbench, window, cx| workbench.run_task(task, window, cx)) else { return false };
        started.push(ended);
    }
    join_all(started).await.into_iter().all(|code| code.ok().flatten() == Some(0))
}

/// `getTask`: a label (the configured name or `source: name`) or a task identifier.
fn find_task(tasks: &[Task], reference: &Value) -> Option<Task> {
    match reference {
        Value::String(label) => tasks.iter().find(|task| task.label == *label || task.name == *label || task.id == *label).cloned(),
        Value::Object(identifier) => {
            let matches = |task: &Task| identifier.iter().all(|(key, value)| key == "_key" || task.definition.get(key) == Some(value));
            tasks.iter().find(|task| task.configures && matches(task)).or_else(|| tasks.iter().find(|task| matches(task))).cloned()
        }
        _ => None,
    }
}

/// `_getGroupedTasks`: configured tasks replace the provided tasks they configure; the rest stay.
fn merge_tasks(configured: Vec<Task>, provided: Vec<Task>) -> Vec<Task> {
    let mut result: Vec<Task> = Vec::new();
    for task in &provided {
        match configured.iter().find(|configuration| configuration.configures_task(task)) {
            Some(configuration) => result.push(task.customize(configuration)),
            None => result.push(task.clone()),
        }
    }
    result.extend(configured.into_iter().filter(|task| !task.configures));
    result
}

/// `_buildShellCommandLine` for a POSIX shell: the command line, or the command and its arguments quoted where
/// they hold spaces.
fn shell_command_line(execution: &Value) -> String {
    if let Some(command_line) = execution["commandLine"].as_str() {
        return command_line.to_owned();
    }
    let text = |value: &Value| value.as_str().map(str::to_owned).unwrap_or_else(|| value["value"].as_str().unwrap_or_default().to_owned());
    let quote = |value: String| if value.contains(' ') && !value.starts_with('\'') && !value.starts_with('"') { format!("'{value}'") } else { value };
    let mut parts = vec![quote(text(&execution["command"]))];
    parts.extend(execution["args"].as_array().into_iter().flatten().map(|arg| quote(text(arg))));
    parts.join(" ")
}
