use serde_json::json;

use super::*;

fn npm_build() -> Task {
    let dto = json!({
        "name": "build",
        "definition": { "type": "npm", "script": "build" },
        "source": { "label": "npm", "extensionId": "vscode.npm" },
        "execution": { "commandLine": "npm run build" },
        "group": { "_id": "build", "isDefault": false },
        "problemMatchers": [],
        "detail": "tsc",
    });
    Task::from_dto(&dto, None, Some(Path::new("/w"))).unwrap()
}

#[test]
fn a_contributed_task_is_customized_with_its_identifier_group_and_label() {
    let (index, element) = customizable_task(&npm_build(), None);
    assert_eq!(index, None);
    assert_eq!(Value::Object(element).to_string(), r#"{"type":"npm","script":"build","group":"build","problemMatcher":[],"label":"npm: build","detail":"tsc"}"#);
}

#[test]
fn a_customized_task_is_found_in_tasks_json_by_its_identifier() {
    let config = json!({ "version": "2.0.0", "tasks": [{ "label": "echo", "type": "shell", "command": "echo" }, { "type": "npm", "script": "build", "group": "build", "label": "npm: build" }] });
    let mut task = npm_build();
    task.source = json!({ "label": "Workspace" });
    let (index, mut element) = customizable_task(&task, Some(&config));
    assert_eq!(index, Some(1));
    element.insert("group".into(), json!({ "kind": "build", "isDefault": true }));
    assert_eq!(element["group"], json!({ "kind": "build", "isDefault": true }));
}

#[test]
fn a_new_tasks_json_starts_with_the_comment_and_follows_insert_spaces() {
    let task = json!({ "type": "npm", "script": "build", "label": "npm: build" });
    let tabs = new_tasks_json(&task, false, 4);
    assert_eq!(tabs, "{\n\t// See https://go.microsoft.com/fwlink/?LinkId=733558 \n\t// for the documentation about the tasks.json format\n\t\"version\": \"2.0.0\",\n\t\"tasks\": [\n\t\t{\n\t\t\t\"type\": \"npm\",\n\t\t\t\"script\": \"build\",\n\t\t\t\"label\": \"npm: build\"\n\t\t}\n\t]\n}");
    let spaces = new_tasks_json(&task, true, 4);
    assert!(spaces.contains("\n            \"type\": \"npm\","), "{spaces}");
}

#[test]
fn the_editor_opens_at_the_entry_lines() {
    let task = json!({ "type": "npm", "script": "build", "label": "npm: build" });
    let content = new_tasks_json(&task, true, 4);
    let (start, end) = entry_selection(&content, &task, 4).unwrap();
    assert_eq!((start.line, start.column, end.line, end.column), (5, 2, 9, 3));
}
