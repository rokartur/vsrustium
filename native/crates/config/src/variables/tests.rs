//! configurationVariables.test.ts and configurationResolverService.test.ts (`Configuration Resolver Service`
//! without the interactive cases, which the workbench tests, and `ConfigurationResolverExpression`).

use super::*;
use serde_json::json;

fn has(values: &[&str]) -> Vec<bool> {
    values.iter().map(|value| has_configuration_variable(value)).collect()
}

#[test]
fn recognizes_complete_variables_including_empty_expressions() {
    let values = ["${name}", "${env:PATH}", "${}", "${:}", "${input:}", "${command:name:argument}", "before ${name} after", "${one}${two}", "$${name}", "\\${name}"];
    assert_eq!(has(&values), vec![true; values.len()]);
}

#[test]
fn recognizes_nested_ordinary_braces_and_variables() {
    let values = ["${{}}", "${{{name}}}", "${command:name{argument}tail}", "${outer:${inner}}", "${outer:{${inner:{argument}}}}"];
    assert_eq!(has(&values), vec![true; values.len()]);
}

#[test]
fn leaves_incomplete_markers_and_ordinary_braces_literal() {
    let values = ["", "literal", "$", "{name}", "$ {name}", "${", "${name", "${name{}", "${{name}", "${{{}}", "${outer:{inner}{tail}", "}${", "}${{name}"];
    assert_eq!(has(&values), vec![false; values.len()]);
}

#[test]
fn finds_complete_inner_variables_after_incomplete_outer_expressions() {
    let values = ["${outer ${inner}", "${outer:{${inner}", "${${}", "${${name{}}", "${outer ${incomplete ${env:PATH} trailing ${"];
    assert_eq!(has(&values), vec![true; values.len()]);
}

#[test]
fn ignores_unmatched_braces_outside_complete_variables() {
    let values = ["}${name}", "${name}{", "${name}}", "{${name}{", "${name} trailing ${"];
    assert_eq!(has(&values), vec![true; values.len()]);
}

#[test]
fn agrees_with_the_parser_on_all_short_brace_expressions() {
    fn check(value: String, remaining: usize, mismatches: &mut Vec<String>) {
        let expected = (0..value.len()).any(|offset| parse_configuration_variable(&value, offset).is_some());
        if has_configuration_variable(&value) != expected {
            mismatches.push(value.clone());
        }
        if remaining > 0 {
            for character in ['$', '{', '}', 'a', ':'] {
                check(format!("{value}{character}"), remaining - 1, mismatches);
            }
        }
    }
    let mut mismatches = Vec::new();
    check(String::new(), 6, &mut mismatches);
    assert_eq!(mismatches, Vec::<String>::new());
}

#[test]
fn scans_64_kib_of_incomplete_markers_within_the_performance_budget() {
    for suffix in ["", "${env:PATH}"] {
        let value = "${".repeat(32 * 1024) + suffix;
        let start = std::time::Instant::now();
        assert_eq!(has_configuration_variable(&value), !suffix.is_empty());
        assert!(start.elapsed().as_millis() < 1000);
    }
}

const WORKSPACE: &str = "/VSCode/workspaceLocation";

/// The suite's context: one folder `workspaceLocation`, an editor on `<folder>/file` with line 10 selected,
/// `key1` and `key2` in the environment.
fn context(config: Value) -> Context {
    Context {
        folders: vec![("workspaceLocation".into(), WORKSPACE.into())],
        config: Box::new(move |_, section| section.split('.').try_fold(&config, |value, key| value.get(key)).cloned()),
        extension_location: Box::new(|id| (id == "publisher.extId").then(|| "/some/path".to_string())),
        file_path: Some(format!("{WORKSPACE}/file")),
        workspace_folder_for_file: Some(WORKSPACE.into()),
        selected_text: None,
        line_number: Some("10".into()),
        column_number: Some("1".into()),
        exec_path: None,
        app_root: None,
        env: HashMap::from([("key1".into(), "Value for key1".into()), ("key2".into(), "Value for key2".into())]),
        user_home: Some("/users/username".into()),
        cwd: "/cwd".into(),
    }
}

fn resolve_text(folder: Option<&str>, text: &str, context: &Context) -> Result<String, VariableError> {
    resolve(folder, &json!(text), context).map(|value| value.as_str().unwrap().to_string())
}

#[test]
fn substitutes_workspace_and_file_variables() {
    let context = context(json!({}));
    let workspace = Some(WORKSPACE);
    assert_eq!(resolve_text(workspace, "abc ${workspaceFolder} xyz", &context).unwrap(), "abc /VSCode/workspaceLocation xyz");
    assert_eq!(resolve_text(workspace, "abc ${workspaceFolder:workspaceLocation} xyz", &context).unwrap(), "abc /VSCode/workspaceLocation xyz");
    assert!(resolve_text(workspace, "abc ${workspaceFolder:invalidLocation} xyz", &context).is_err());
    assert!(resolve_text(None, "abc ${workspaceFolder} xyz", &context).is_err());
    assert_eq!(resolve_text(None, "abc ${workspaceFolder:workspaceLocation} xyz", &context).unwrap(), "abc /VSCode/workspaceLocation xyz");
    assert!(resolve_text(None, "abc ${workspaceFolder:invalidLocation} xyz", &context).is_err());
    assert_eq!(resolve_text(workspace, "abc ${workspaceRootFolderName} xyz", &context).unwrap(), "abc workspaceLocation xyz");
    assert_eq!(resolve_text(workspace, "abc ${lineNumber} xyz", &context).unwrap(), "abc 10 xyz");
    assert_eq!(resolve_text(workspace, "abc ${relativeFile} xyz", &context).unwrap(), "abc file xyz");
    assert_eq!(resolve_text(workspace, "abc ${relativeFile:workspaceLocation} xyz", &context).unwrap(), "abc file xyz");
    assert!(resolve_text(workspace, "abc ${relativeFile:invalidLocation} xyz", &context).is_err());
    assert_eq!(resolve_text(None, "abc ${relativeFile} xyz", &context).unwrap(), "abc /VSCode/workspaceLocation/file xyz");
    assert_eq!(resolve_text(None, "abc ${relativeFile:workspaceLocation} xyz", &context).unwrap(), "abc file xyz");
    assert!(resolve_text(None, "abc ${relativeFile:invalidLocation} xyz", &context).is_err());
    assert_eq!(resolve_text(workspace, "${workspaceFolder} - ${workspaceFolder}", &context).unwrap(), "/VSCode/workspaceLocation - /VSCode/workspaceLocation");
    assert_eq!(resolve_text(workspace, "${extensionInstallFolder:publisher.extId}", &context).unwrap(), "/some/path");
}

#[test]
fn file_variables_follow_node_path_rules() {
    let mut context = context(json!({}));
    context.file_path = Some(format!("{WORKSPACE}/src/main.test.rs"));
    let workspace = Some(WORKSPACE);
    assert_eq!(resolve_text(workspace, "${relativeFileDirname}", &context).unwrap(), "src");
    assert_eq!(resolve_text(workspace, "${fileDirname}", &context).unwrap(), "/VSCode/workspaceLocation/src");
    assert_eq!(resolve_text(workspace, "${fileExtname}", &context).unwrap(), ".rs");
    assert_eq!(resolve_text(workspace, "${fileBasename}", &context).unwrap(), "main.test.rs");
    assert_eq!(resolve_text(workspace, "${fileBasenameNoExtension}", &context).unwrap(), "main.test");
    assert_eq!(resolve_text(workspace, "${fileDirnameBasename}", &context).unwrap(), "src");
    assert_eq!(resolve_text(workspace, "${fileWorkspaceFolderBasename}", &context).unwrap(), "workspaceLocation");
    assert_eq!(resolve_text(workspace, "${pathSeparator}${/}${cwd}${userHome}", &context).unwrap(), "///VSCode/workspaceLocation/users/username");
    assert_eq!(resolve_text(None, "${cwd} ${execPath}", &context).unwrap(), "/cwd ${execPath}");
    context.file_path = Some(format!("{WORKSPACE}/file"));
    assert_eq!(resolve_text(workspace, "${relativeFileDirname}", &context).unwrap(), ".");
    assert!(resolve_text(workspace, "${selectedText}", &context).is_err());
}

#[test]
fn substitutes_env_variables() {
    let context = context(json!({}));
    let workspace = Some(WORKSPACE);
    assert_eq!(resolve_text(workspace, "abc ${workspaceFolder} ${env:key1} xyz", &context).unwrap(), "abc /VSCode/workspaceLocation Value for key1 xyz");
    assert_eq!(
        resolve_text(workspace, "${workspaceFolder} - ${workspaceFolder} ${env:key1} - ${env:key2}", &context).unwrap(),
        "/VSCode/workspaceLocation - /VSCode/workspaceLocation Value for key1 - Value for key2"
    );
    // disallows nested keys (#77289)
    assert_eq!(resolve_text(workspace, "${env:key1} ${env:key1${env:key2}}", &context).unwrap(), "Value for key1 ");
    // platform case sensitivity
    assert_eq!(resolve_text(workspace, "${env:key1} - ${env:Key1}", &context).unwrap(), "Value for key1 - ");
}

#[test]
fn substitutes_configuration_variables() {
    let context = context(json!({
        "editor": { "fontFamily": "foo", "lineNumbers": 123, "insertSpaces": false, "list": ["foo", "bar"] },
        "terminal": { "integrated": { "fontFamily": "bar" } },
        "key1": "key1=${config:key2}",
        "key2": "key2=${config:key3}",
        "key3": "we did it!",
    }));
    let workspace = Some(WORKSPACE);
    assert_eq!(resolve_text(workspace, "abc ${config:editor.fontFamily} xyz", &context).unwrap(), "abc foo xyz");
    // inlines an array (#245718)
    assert_eq!(resolve_text(workspace, "abc ${config:editor.list} xyz", &context).unwrap(), "abc foo,bar xyz");
    assert_eq!(resolve_text(None, "abc ${config:editor.fontFamily} xyz", &context).unwrap(), "abc foo xyz");
    assert_eq!(resolve_text(workspace, "abc ${config:editor.fontFamily} ${config:terminal.integrated.fontFamily} xyz", &context).unwrap(), "abc foo bar xyz");
    assert_eq!(
        resolve_text(workspace, "abc ${config:editor.fontFamily} ${workspaceFolder} ${env:key1} xyz", &context).unwrap(),
        "abc foo /VSCode/workspaceLocation Value for key1 xyz"
    );
    assert_eq!(resolve_text(workspace, "${config:key1}", &context).unwrap(), "key1=key2=we did it!");
    assert_eq!(resolve_text(workspace, "abc ${config:editor.fontFamily} ${config:editor.lineNumbers} ${config:editor.insertSpaces} xyz", &context).unwrap(), "abc foo 123 false xyz");
    // uses original variable as fallback
    assert_eq!(resolve_text(workspace, "abc ${unknownVariable} xyz", &context).unwrap(), "abc ${unknownVariable} xyz");
    assert_eq!(resolve_text(workspace, "abc ${env:unknownVariable} xyz", &context).unwrap(), "abc  xyz");
}

#[test]
fn configuration_variables_with_invalid_accessor() {
    let context = context(json!({ "editor": { "fontFamily": "foo" } }));
    for text in ["abc ${env} xyz", "abc ${env:} xyz", "abc ${config} xyz", "abc ${config:} xyz", "abc ${config:editor} xyz", "abc ${config:editor..fontFamily} xyz", "abc ${config:editor.none.none2} xyz"] {
        assert!(resolve_text(Some(WORKSPACE), text, &context).is_err(), "{text}");
    }
}

#[test]
fn platform_specific_config() {
    let context = context(json!({}));
    let config = resolve(Some(WORKSPACE), &json!({ "program": "osx.sh", "windows": { "program": "windows.exe" }, "linux": { "program": "linux.sh" } }), &context).unwrap();
    assert_eq!(config, json!({ "program": "osx.sh" }));
    let original = json!({ "windows": { "program": "windows.exe" }, "osx": { "program": "osx.sh" }, "linux": { "program": "linux.sh" } });
    let config = resolve(Some(WORKSPACE), &original, &context).unwrap();
    assert_eq!(config, json!({ "program": "osx.sh" }));
    assert_eq!(original["osx"]["program"], "osx.sh");
}

#[test]
fn resolve_with_environment_uses_the_given_environment() {
    let context = context(json!({}));
    let env = HashMap::from([("VAR_1".into(), "VAL_1".into()), ("VAR_2".into(), "VAL_2".into())]);
    assert_eq!(resolve_with_environment(&env, None, "echo ${env:VAR_1}${env:VAR_2}", &context).unwrap(), "echo VAL_1VAL_2");
    assert!(resolve_with_environment(&env, None, "${userHome}", &context).is_err());
}

#[test]
fn substitution_in_object_key() {
    let context = context(json!({}));
    let config = json!({ "name": "Test", "mappings": { "pos1": "value1", "${workspaceFolder}/test1": "${workspaceFolder}/test2", "pos3": "value3" } });
    assert_eq!(
        resolve(Some(WORKSPACE), &config, &context).unwrap(),
        json!({ "name": "Test", "mappings": { "pos1": "value1", "/VSCode/workspaceLocation/test1": "/VSCode/workspaceLocation/test2", "pos3": "value3" } })
    );
}

fn variable(id: &str) -> Variable {
    parse_configuration_variable(id, 0).unwrap().0
}

#[test]
fn expression_parses_variables() {
    let expression = Expression::parse(&json!({}));
    assert!(expression.unresolved().is_empty());
    assert_eq!(expression.to_object(), json!({}));

    let unresolved = Expression::parse(&json!({ "value": "${env:HOME}" })).unresolved();
    assert_eq!((unresolved.len(), unresolved[0].name.as_str(), unresolved[0].arg.as_deref()), (1, "env", Some("HOME")));

    let unresolved = Expression::parse(&json!({ "value": "${config:path:to:value}" })).unresolved();
    assert_eq!((unresolved.len(), unresolved[0].name.as_str(), unresolved[0].arg.as_deref()), (1, "config", Some("path:to:value")));

    let expression = Expression::parse(&json!({
        "name": "${env:USERNAME}",
        "path": "${env:HOME}/folder",
        "settings": { "value": "${config:path}" },
        "array": ["${env:TERM}", { "key": "${env:KEY}" }],
    }));
    let mut names: Vec<String> = expression.unresolved().into_iter().map(|variable| variable.name).collect();
    names.sort();
    assert_eq!(names, ["config", "env", "env", "env", "env"]);

    let unresolved = Expression::parse(&json!("abc ${env:HOME${env:USER}} xyz")).unresolved();
    assert_eq!((unresolved.len(), unresolved[0].arg.as_deref()), (1, Some("HOME${env:USER}")));
}

#[test]
fn expression_resolves_and_keeps_unresolved_variables() {
    let mut expression = Expression::parse(&json!({ "name": "${env:USERNAME}", "path": "${env:HOME}/folder" }));
    assert_eq!(expression.to_object(), json!({ "name": "${env:USERNAME}", "path": "${env:HOME}/folder" }));
    expression.resolve(&variable("${env:USERNAME}"), "testuser".to_string());
    expression.resolve(&variable("${env:HOME}"), "/home/testuser".to_string());
    assert_eq!(expression.to_object(), json!({ "name": "testuser", "path": "/home/testuser/folder" }));

    let mut expression = Expression::parse(&json!({ "first": "${env:HOME}", "second": "${env:HOME}" }));
    let unresolved = expression.unresolved();
    assert_eq!(unresolved.len(), 1);
    expression.resolve(&unresolved[0], "/home/user".to_string());
    assert_eq!(expression.to_object(), json!({ "first": "/home/user", "second": "/home/user" }));

    let mut expression = Expression::parse(&json!("abc ${env:HOME} xyz"));
    expression.resolve(&variable("${env:HOME}"), "/home/user".to_string());
    assert_eq!(expression.to_object(), json!("abc /home/user xyz"));

    let mut expression = Expression::parse(&json!("${env:HOME}/folder${env:SHELL}"));
    assert_eq!(expression.unresolved().len(), 2);
    expression.resolve(&variable("${env:HOME}"), "/home/user".to_string());
    expression.resolve(&variable("${env:SHELL}"), "/bin/bash".to_string());
    assert_eq!(expression.to_object(), json!("/home/user/folder/bin/bash"));
}

/// The `for (const r of expr.unresolved())` loop: variables a resolved value brings in are visited too.
fn resolve_each(expression: &mut Expression, mut value_of: impl FnMut(&Variable) -> Option<String>) {
    let mut seen = HashSet::new();
    while let Some(variable) = expression.unresolved().into_iter().find(|variable| seen.insert(variable.id.clone())) {
        if let Some(value) = value_of(&variable) {
            expression.resolve(&variable, value);
        }
    }
}

#[test]
fn expression_resolves_nested_values() {
    let mut expression = Expression::parse(&json!({ "name": "${env:REDIRECTED}", "key that is ${env:REDIRECTED}": "cool!" }));
    resolve_each(&mut expression, |variable| match variable.arg.as_deref() {
        Some("REDIRECTED") => Some("username: ${env:USERNAME}".into()),
        Some("USERNAME") => Some("testuser".into()),
        _ => None,
    });
    assert_eq!(expression.to_object(), json!({ "name": "username: testuser", "key that is username: testuser": "cool!" }));

    // #245798
    let mut expression = Expression::parse(&json!({ "env": { "SITE": "${input:site}", "TLD": "${input:tld}", "HOST": "${input:host}" } }));
    resolve_each(&mut expression, |variable| match variable.arg.as_deref() {
        Some("site") => Some("example".into()),
        Some("tld") => Some("com".into()),
        Some("host") => Some("local.${input:site}.${input:tld}".into()),
        _ => None,
    });
    assert_eq!(expression.to_object(), json!({ "env": { "SITE": "example", "TLD": "com", "HOST": "local.example.com" } }));
}

#[test]
fn expression_out_of_order_key_resolution() {
    // #248550
    let mut expression = Expression::parse(&json!({ "${input:key}": "${input:value}" }));
    resolve_each(&mut expression, |variable| (variable.arg.as_deref() == Some("key")).then(|| "the-key".into()));
    resolve_each(&mut expression, |variable| (variable.arg.as_deref() == Some("value")).then(|| "the-value".into()));
    assert_eq!(expression.to_object(), json!({ "the-key": "the-value" }));
}

#[test]
fn expression_keeps_a_variable_inside_its_own_value() {
    let mut expression = Expression::parse(&json!("${env:A}"));
    resolve_each(&mut expression, |_| Some("a ${env:A}".into()));
    assert_eq!(expression.to_object(), json!("a ${env:A}"));
}
