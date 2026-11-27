use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use config::workspace::{Workspace, WorkspaceFolder};
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::editor::{Position, Selection};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn string_arguments_resolve_against_the_active_file_and_its_folder(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark 2026").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-resolver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project/src")).unwrap();
    std::fs::write(dir.join("project/src/main.ts"), "let answer = 42;\n").unwrap();
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), "{ \"editor.tabSize\": 2 }").unwrap();
    let folder = dir.join("project");
    let workspace = Workspace { file: None, folders: vec![WorkspaceFolder { path: folder.clone(), name: "project".into() }], transient: false };
    let settings = Arc::new(Mutex::new(Settings::load(Registry::core(), dir.join("User"), Some(workspace.clone()))));
    let (sender, _messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx));

    workbench.update_in(cx, |workbench, window, cx| {
        workbench.open_file(folder.join("src/main.ts"), true, window, cx);
        let editor = workbench.editors.active_mut().unwrap();
        editor.selections = vec![Selection { anchor: Position::new(0, 4), head: Position::new(0, 10) }];

        let args = json!({
            "query": "${selectedText} at ${lineNumber}:${columnNumber}",
            "filesToInclude": "${workspaceFolder}/src,${relativeFileDirname}",
            "filesToExclude": "${workspaceFolderBasename} ${config:editor.tabSize}",
            "isRegexp": true,
        });
        let resolved = workbench.resolve_string_arguments(args).unwrap();
        assert_eq!(resolved["query"], "answer at 1:11");
        assert_eq!(resolved["filesToInclude"], format!("{}/src,src", folder.display()));
        assert_eq!(resolved["filesToExclude"], "project 2");
        assert_eq!(resolved["isRegexp"], true);

        let error = workbench.resolve_string_arguments(json!({ "query": "${workspaceFolder:missing}" })).unwrap_err();
        assert_eq!(error.message, "Variable workspaceFolder can not be resolved. No such folder 'missing'.");
    });
    std::fs::remove_dir_all(dir).unwrap();
}

/// Accepts the shown quick input as Enter does: the active item, or the typed value.
fn accept_quick_input(workbench: &gpui::Entity<Workbench>, cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, cx| {
        let id = workbench.quick_inputs.current().expect("a prompt shows").id;
        workbench.quick_inputs.get_mut(id).unwrap().accept_from_ui();
        workbench.flush_quick_input_events(window, cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn interactive_variables_ask_in_order_and_remember_the_last_values(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark 2026").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-interactive-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::core(), dir.join("User"), None)));
    let (sender, _messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx));
    workbench.update(cx, |workbench, _| {
        // `${command:}` gets the configuration resolved so far.
        workbench.commands.register("test.soFar", |_, args, _, _| crate::commands::Outcome::Ready(Ok(json!(format!("after {}", args[0][0].as_str().unwrap())))));
        workbench.commands.register("test.args", |_, args, _, _| crate::commands::Outcome::Ready(Ok(args[0]["v"].clone())));
    });
    let inputs = vec![
        json!({ "id": "pick", "type": "pickString", "description": "Pick", "options": ["a", { "label": "Bee", "value": "b" }], "default": "b" }),
        json!({ "id": "name", "type": "promptString", "description": "Name", "default": "x" }),
        json!({ "id": "cmd", "type": "command", "command": "test.args", "args": { "v": "z" } }),
    ];
    let config = json!(["${input:pick}", "${command:soFar}", "${input:name}", "${input:cmd}"]);
    let commands = json!({ "soFar": "test.soFar" }).as_object().unwrap().clone();
    let result = std::rc::Rc::new(std::cell::RefCell::new(None));
    let run = |cx: &mut gpui::VisualTestContext| {
        let (config, inputs, commands, result) = (config.clone(), inputs.clone(), commands.clone(), result.clone());
        workbench.update_in(cx, |_, window, cx| {
            cx.spawn_in(window, async move |this, cx| {
                let resolved = super::resolve_with_interaction(&this, cx, &config, "tasks", Some(&inputs), &commands).await;
                *result.borrow_mut() = Some(resolved);
            })
            .detach();
        });
    };

    run(cx);
    cx.run_until_parked();
    let picks = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().rows.iter().map(|row| (row.item.label.clone(), row.item.description.clone())).collect::<Vec<_>>());
    assert_eq!(picks, [("Bee: b".to_owned(), Some("(Default)".to_owned())), ("a".to_owned(), None)]);
    accept_quick_input(&workbench, cx);
    let prompt = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().map(|input| (input.prompt.clone(), input.input_box.value.clone(), input.ignore_focus_out)));
    assert_eq!(prompt, Some((Some("Name".into()), "x".into(), true)));
    workbench.update(cx, |workbench, _| {
        let id = workbench.quick_inputs.current().unwrap().id;
        workbench.quick_inputs.get_mut(id).unwrap().set_value("typed".into());
    });
    accept_quick_input(&workbench, cx);
    let resolved = result.borrow_mut().take().expect("resolved").unwrap().unwrap();
    let resolved: Vec<(&str, &str)> = resolved.iter().map(|(variable, value)| (variable.as_str(), value.as_str())).collect();
    assert_eq!(resolved, [("${input:pick}", "b"), ("${command:soFar}", "after b"), ("${input:name}", "typed"), ("${input:cmd}", "z")]);

    // The next run offers the last typed value; a cancelled prompt cancels the whole flow.
    run(cx);
    accept_quick_input(&workbench, cx);
    let value = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().input_box.value.clone());
    assert_eq!(value, "typed");
    workbench.update_in(cx, |workbench, window, cx| {
        let id = workbench.quick_inputs.current().unwrap().id;
        workbench.hide_quick_input_by_id(id, window, cx);
        workbench.flush_quick_input_events(window, cx);
    });
    cx.run_until_parked();
    assert_eq!(result.borrow_mut().take().expect("resolved"), Ok(None));
    let stored = workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get("configResolveInputLru").map(str::to_owned));
    // The second run's pick moved its value to the newest end.
    assert_eq!(stored.as_deref(), Some(r#"[["tasks.name","typed"],["tasks.pick","b"]]"#));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_input_lru_keeps_the_five_newest() {
    let lru = (0..7).fold(Vec::new(), |lru, index| super::with_input(lru, &format!("tasks.{index}"), "v"));
    assert_eq!(lru.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["tasks.2", "tasks.3", "tasks.4", "tasks.5", "tasks.6"]);
    let lru = super::with_input(lru, "tasks.3", "w");
    assert_eq!(lru.last().unwrap(), &("tasks.3".to_owned(), "w".to_owned()));
    assert_eq!(lru.len(), 5);
}

#[test]
fn a_task_asks_for_its_command_line_or_its_custom_definition() {
    let shell = crate::tasks::configuration::Task { execution: Some(json!({ "command": "echo ${input:a}", "args": ["${command:b}"] })), definition: json!({ "type": "shell", "x": "${input:ignored}" }), ..Default::default() };
    assert_eq!(super::task_interactive_variables(&shell), Some(json!(["${input:a}", "${command:b}"])));
    let custom = crate::tasks::configuration::Task { execution: Some(json!({ "customExecution": "customExecution" })), definition: json!({ "type": "${input:no}", "_key": "k", "flag": "${input:c}" }), ..Default::default() };
    assert_eq!(super::task_interactive_variables(&custom), Some(json!(["${input:c}"])));
    let plain = crate::tasks::configuration::Task { execution: Some(json!({ "command": "make" })), ..Default::default() };
    assert_eq!(super::task_interactive_variables(&plain), None);
    let replaced = super::task_with_variables(shell, &[("${input:a}".into(), "1".into()), ("${command:b}".into(), "2".into())]);
    assert_eq!(replaced.execution, Some(json!({ "command": "echo 1", "args": ["2"] })));
}
