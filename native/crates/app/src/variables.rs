#![allow(dead_code, reason = "tasks and terminal callers are not wired yet")]

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Ports the `IVariableResolveContext` data plus the environment and user home that
/// `AbstractVariableResolverService` awaits. `folder` is the `folderUri` passed to `resolveAsync`.
#[derive(Clone, Debug, Default)]
pub struct VariableContext {
    pub folders: Vec<(String, PathBuf)>,
    pub folder: Option<PathBuf>,
    pub file: Option<PathBuf>,
    pub line_number: Option<u32>,
    pub column_number: Option<u32>,
    pub selected_text: Option<String>,
    pub exec_path: Option<PathBuf>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub home: Option<PathBuf>,
}

/// Ports `AbstractVariableResolverService.resolveAsync` for a string config.
pub fn resolve(
    text: &str,
    context: &VariableContext,
    config: &dyn Fn(&str) -> Option<Value>,
) -> Result<String, String> {
    expand(text, context, config, &mut Vec::new())
}

/// Ports `AbstractVariableResolverService.resolveAsync` for any config: applies the `osx`
/// platform keys of the root object like `ConfigurationResolverExpression.applyPlatformSpecificKeys`,
/// then resolves every string value and object key.
pub fn resolve_value(
    value: &Value,
    context: &VariableContext,
    config: &dyn Fn(&str) -> Option<Value>,
) -> Result<Value, String> {
    let Value::Object(root) = value else {
        return resolve_any(value, context, config);
    };
    let mut root = root.clone();
    if let Some(Value::Object(platform)) = root.get("osx").cloned() {
        for (key, value) in platform {
            root.insert(key, value);
        }
    }
    for key in ["windows", "osx", "linux"] {
        root.shift_remove(key);
    }
    resolve_any(&Value::Object(root), context, config)
}

/// Ports `ConfigurationResolverExpression.parseObject`: strings in arrays, object keys and values.
fn resolve_any(
    value: &Value,
    context: &VariableContext,
    config: &dyn Fn(&str) -> Option<Value>,
) -> Result<Value, String> {
    match value {
        Value::String(text) => Ok(Value::String(resolve(text, context, config)?)),
        Value::Array(items) => {
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(resolve_any(item, context, config)?);
            }
            Ok(Value::Array(resolved))
        }
        Value::Object(map) => {
            let mut resolved = serde_json::Map::new();
            for (key, item) in map {
                resolved.insert(
                    resolve(key, context, config)?,
                    resolve_any(item, context, config)?,
                );
            }
            Ok(Value::Object(resolved))
        }
        other => Ok(other.clone()),
    }
}

/// Ports `ConfigurationResolverExpression.parseString` and `_resolveAtLocation`: a resolved value is
/// parsed again for variables, except the ones on `path`, which stops `${env:A}` -> `${env:A}` loops.
fn expand(
    text: &str,
    context: &VariableContext,
    config: &dyn Fn(&str) -> Option<Value>,
    path: &mut Vec<String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut pos = 0;
    while let Some(offset) = text[pos..].find("${") {
        let start = pos + offset;
        let Some(end) = closing_brace(text, start) else {
            out.push_str(&text[pos..start + 2]);
            pos = start + 2;
            continue;
        };
        out.push_str(&text[pos..start]);
        pos = end + 1;
        let id = &text[start..pos];
        if path.iter().any(|seen| seen == id) {
            out.push_str(id);
            continue;
        }
        let value = evaluate_single_variable(id, context, config)?;
        if value == id {
            out.push_str(id);
            continue;
        }
        path.push(id.to_string());
        let expanded = expand(&value, context, config, path);
        path.pop();
        out.push_str(&expanded?);
    }
    out.push_str(&text[pos..]);
    Ok(out)
}

/// Ports `parseConfigurationVariable`: the index of the `}` closing the `${` at `start`, counting nested braces.
fn closing_brace(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0;
    for (index, byte) in text.bytes().enumerate().skip(start + 1) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Ports `AbstractVariableResolverService.evaluateSingleVariable` without a command value mapping:
/// `${command:...}`, `${input:...}`, contributed and unknown variables come back as `id`.
fn evaluate_single_variable(
    id: &str,
    context: &VariableContext,
    config: &dyn Fn(&str) -> Option<Value>,
) -> Result<String, String> {
    let inner = &id[2..id.len() - 1];
    let (variable, argument) = match inner.split_once(':') {
        Some((name, arg)) => (name, Some(arg).filter(|arg| !arg.is_empty())),
        None => (inner, None),
    };

    let file_path = || -> Result<String, String> {
        match &context.file {
            Some(file) => Ok(file.to_string_lossy().into_owned()),
            None => Err(format!(
                "Variable {id} can not be resolved. Please open an editor."
            )),
        }
    };
    let folder_path_for_file = || -> Result<String, String> {
        let file = file_path()?;
        match workspace_folder_for_file(&context.folders, Path::new(&file)) {
            Some(folder) => Ok(folder.to_string_lossy().into_owned()),
            None => Err(format!(
                "Variable {id}: can not find workspace folder of '{}'.",
                basename(&file)
            )),
        }
    };
    let folder_path = |kind: &str| -> Result<String, String> {
        if let Some(name) = argument {
            return match context
                .folders
                .iter()
                .find(|(folder_name, _)| folder_name == name)
            {
                Some((_, path)) => Ok(path.to_string_lossy().into_owned()),
                None => Err(format!(
                    "Variable {kind} can not be resolved. No such folder '{name}'."
                )),
            };
        }
        if let Some(folder) = &context.folder {
            return Ok(folder.to_string_lossy().into_owned());
        }
        if context.folders.len() > 1 {
            return Err(format!(
                "Variable {kind} can not be resolved in a multi folder workspace. Scope this variable using ':' and a workspace folder name."
            ));
        }
        Err(format!(
            "Variable {kind} can not be resolved. Please open a folder."
        ))
    };
    let has_folder = context.folder.is_some() || argument.is_some();

    match variable {
        "env" => {
            let Some(name) = argument else {
                return Err(format!(
                    "Variable {id} can not be resolved because no environment variable name is given."
                ));
            };
            Ok(context.env.get(name).cloned().unwrap_or_default())
        }
        "config" => {
            let Some(section) = argument else {
                return Err(format!(
                    "Variable {id} can not be resolved because no settings name is given."
                ));
            };
            match config(section) {
                None | Some(Value::Null) => Err(format!(
                    "Variable {id} can not be resolved because setting '{section}' not found."
                )),
                Some(Value::Object(_)) => Err(format!(
                    "Variable {id} can not be resolved because '{section}' is a structured value."
                )),
                Some(value) => Ok(js_string(&value)),
            }
        }
        "workspaceRoot" | "workspaceFolder" => folder_path("workspaceFolder"),
        "cwd" if !has_folder => Ok(context.cwd.to_string_lossy().into_owned()),
        "cwd" => folder_path("cwd"),
        "workspaceRootFolderName" | "workspaceFolderBasename" => {
            Ok(basename(&folder_path("workspaceFolderBasename")?).to_string())
        }
        "userHome" => match &context.home {
            Some(home) => Ok(home.to_string_lossy().into_owned()),
            None => Err(format!(
                "Variable {id} can not be resolved. UserHome path is not defined"
            )),
        },
        "lineNumber" => match context.line_number {
            Some(line) => Ok(line.to_string()),
            None => Err(format!(
                "Variable {id} can not be resolved. Make sure to have a line selected in the active editor."
            )),
        },
        "columnNumber" => match context.column_number {
            Some(column) => Ok(column.to_string()),
            None => Err(format!(
                "Variable {id} can not be resolved. Make sure to have a column selected in the active editor."
            )),
        },
        "selectedText" => match context.selected_text.as_deref() {
            Some(text) if !text.is_empty() => Ok(text.to_string()),
            _ => Err(format!(
                "Variable {id} can not be resolved. Make sure to have some text selected in the active editor."
            )),
        },
        "file" => file_path(),
        "fileWorkspaceFolder" => folder_path_for_file(),
        "fileWorkspaceFolderBasename" => Ok(basename(&folder_path_for_file()?).to_string()),
        "relativeFile" if has_folder => {
            let folder = folder_path("relativeFile")?;
            Ok(relative(&folder, &file_path()?))
        }
        "relativeFile" => file_path(),
        "relativeFileDirname" => {
            let dir = dirname(&file_path()?);
            if !has_folder {
                return Ok(dir);
            }
            let relative = relative(&folder_path("relativeFileDirname")?, &dir);
            Ok(if relative.is_empty() {
                ".".to_string()
            } else {
                relative
            })
        }
        "fileDirname" => Ok(dirname(&file_path()?)),
        "fileExtname" => Ok(extname(basename(&file_path()?)).to_string()),
        "fileBasename" => Ok(basename(&file_path()?).to_string()),
        "fileBasenameNoExtension" => {
            let file = file_path()?;
            let name = basename(&file);
            Ok(name[..name.len() - extname(name).len()].to_string())
        }
        "fileDirnameBasename" => Ok(basename(&dirname(&file_path()?)).to_string()),
        "execPath" => Ok(context.exec_path.as_ref().map_or_else(
            || id.to_string(),
            |path| path.to_string_lossy().into_owned(),
        )),
        "pathSeparator" | "/" => Ok("/".to_string()),
        _ => Ok(id.to_string()),
    }
}

/// Ports `IWorkspaceContextService.getWorkspaceFolder`: the most specific folder containing `file`.
fn workspace_folder_for_file<'a>(
    folders: &'a [(String, PathBuf)],
    file: &Path,
) -> Option<&'a PathBuf> {
    folders
        .iter()
        .map(|(_, path)| path)
        .filter(|path| file.starts_with(path))
        .max_by_key(|path| path.components().count())
}

/// Ports JavaScript `String(value)` for the non-object values `${config:...}` accepts.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
        other => other.to_string(),
    }
}

/// Ports posix `paths.basename`.
fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// Ports posix `paths.dirname`.
fn dirname(path: &str) -> String {
    match Path::new(path).parent() {
        Some(parent) if parent.as_os_str().is_empty() => ".".to_string(),
        Some(parent) => parent.to_string_lossy().into_owned(),
        None if path.starts_with('/') => "/".to_string(),
        None => ".".to_string(),
    }
}

/// Ports posix `paths.extname` for a basename.
fn extname(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[dot..],
        _ => "",
    }
}

/// Ports posix `paths.relative` for absolute paths.
fn relative(from: &str, to: &str) -> String {
    let from: Vec<Component> = Path::new(from).components().collect();
    let to: Vec<Component> = Path::new(to).components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|part| part.as_os_str().to_str().unwrap_or("")),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context(folder: bool) -> VariableContext {
        let location = PathBuf::from("/VSCode/workspaceLocation");
        VariableContext {
            folders: vec![("workspaceLocation".to_string(), location.clone())],
            folder: folder.then_some(location),
            file: Some(PathBuf::from("/VSCode/workspaceLocation/file")),
            line_number: Some(10),
            column_number: Some(10),
            env: HashMap::from([
                ("key1".to_string(), "Value for key1".to_string()),
                ("key2".to_string(), "Value for key2".to_string()),
            ]),
            ..VariableContext::default()
        }
    }

    /// Mirrors `TestConfigurationService.getValue` for a dotted section.
    fn settings(root: Value) -> impl Fn(&str) -> Option<Value> {
        move |section| {
            let mut value = &root;
            for key in section.split('.') {
                value = value.get(key)?;
            }
            Some(value.clone())
        }
    }

    fn no_config(_: &str) -> Option<Value> {
        None
    }

    fn run(text: &str, folder: bool) -> Result<String, String> {
        resolve(text, &context(folder), &no_config)
    }

    #[test]
    fn substitute_one() {
        assert_eq!(
            run("abc ${workspaceFolder} xyz", true).unwrap(),
            "abc /VSCode/workspaceLocation xyz"
        );
    }

    #[test]
    fn does_not_preserve_platform_config_even_when_not_matched() {
        let config = json!({ "program": "osx.sh", "windows": { "program": "windows.exe" }, "linux": { "program": "linux.sh" } });
        let resolved = resolve_value(&config, &context(true), &no_config).unwrap();
        assert_eq!(resolved, json!({ "program": "osx.sh" }));
    }

    #[test]
    fn apples_platform_specific_config() {
        let config = json!({ "windows": { "program": "windows.exe" }, "osx": { "program": "osx.sh" }, "linux": { "program": "linux.sh" } });
        let original = config.clone();
        let resolved = resolve_value(&config, &context(true), &no_config).unwrap();
        assert_eq!(resolved, json!({ "program": "osx.sh" }));
        assert_eq!(config, original);
    }

    #[test]
    fn workspace_folder_with_argument() {
        assert_eq!(
            run("abc ${workspaceFolder:workspaceLocation} xyz", true).unwrap(),
            "abc /VSCode/workspaceLocation xyz"
        );
    }

    #[test]
    fn workspace_folder_with_invalid_argument() {
        assert_eq!(
            run("abc ${workspaceFolder:invalidLocation} xyz", true).unwrap_err(),
            "Variable workspaceFolder can not be resolved. No such folder 'invalidLocation'."
        );
    }

    #[test]
    fn workspace_folder_with_undefined_workspace_folder() {
        assert_eq!(
            run("abc ${workspaceFolder} xyz", false).unwrap_err(),
            "Variable workspaceFolder can not be resolved. Please open a folder."
        );
        let mut multi_root = context(false);
        multi_root
            .folders
            .push(("other".to_string(), PathBuf::from("/other")));
        assert_eq!(
            resolve("${workspaceRoot}", &multi_root, &no_config).unwrap_err(),
            "Variable workspaceFolder can not be resolved in a multi folder workspace. Scope this variable using ':' and a workspace folder name."
        );
    }

    #[test]
    fn workspace_folder_with_argument_and_undefined_workspace_folder() {
        assert_eq!(
            run("abc ${workspaceFolder:workspaceLocation} xyz", false).unwrap(),
            "abc /VSCode/workspaceLocation xyz"
        );
    }

    #[test]
    fn workspace_folder_with_invalid_argument_and_undefined_workspace_folder() {
        assert!(run("abc ${workspaceFolder:invalidLocation} xyz", false).is_err());
    }

    #[test]
    fn workspace_root_folder_name() {
        assert_eq!(
            run("abc ${workspaceRootFolderName} xyz", true).unwrap(),
            "abc workspaceLocation xyz"
        );
        assert_eq!(
            run("abc ${workspaceFolderBasename} xyz", true).unwrap(),
            "abc workspaceLocation xyz"
        );
    }

    #[test]
    fn current_selected_line_number() {
        assert_eq!(run("abc ${lineNumber} xyz", true).unwrap(), "abc 10 xyz");
    }

    #[test]
    fn relative_file() {
        assert_eq!(
            run("abc ${relativeFile} xyz", true).unwrap(),
            "abc file xyz"
        );
    }

    #[test]
    fn relative_file_with_argument() {
        assert_eq!(
            run("abc ${relativeFile:workspaceLocation} xyz", true).unwrap(),
            "abc file xyz"
        );
    }

    #[test]
    fn relative_file_with_invalid_argument() {
        assert_eq!(
            run("abc ${relativeFile:invalidLocation} xyz", true).unwrap_err(),
            "Variable relativeFile can not be resolved. No such folder 'invalidLocation'."
        );
    }

    #[test]
    fn relative_file_with_undefined_workspace_folder() {
        assert_eq!(
            run("abc ${relativeFile} xyz", false).unwrap(),
            "abc /VSCode/workspaceLocation/file xyz"
        );
    }

    #[test]
    fn relative_file_with_argument_and_undefined_workspace_folder() {
        assert_eq!(
            run("abc ${relativeFile:workspaceLocation} xyz", false).unwrap(),
            "abc file xyz"
        );
    }

    #[test]
    fn relative_file_with_invalid_argument_and_undefined_workspace_folder() {
        assert!(run("abc ${relativeFile:invalidLocation} xyz", false).is_err());
    }

    #[test]
    fn substitute_many() {
        assert_eq!(
            run("${workspaceFolder} - ${workspaceFolder}", true).unwrap(),
            "/VSCode/workspaceLocation - /VSCode/workspaceLocation"
        );
    }

    #[test]
    fn substitute_one_env_variable() {
        assert_eq!(
            run("abc ${workspaceFolder} ${env:key1} xyz", true).unwrap(),
            "abc /VSCode/workspaceLocation Value for key1 xyz"
        );
    }

    #[test]
    fn substitute_many_env_variable() {
        assert_eq!(
            run(
                "${workspaceFolder} - ${workspaceFolder} ${env:key1} - ${env:key2}",
                true
            )
            .unwrap(),
            "/VSCode/workspaceLocation - /VSCode/workspaceLocation Value for key1 - Value for key2"
        );
    }

    #[test]
    fn disallows_nested_keys_77289() {
        assert_eq!(
            run("${env:key1} ${env:key1${env:key2}}", true).unwrap(),
            "Value for key1 "
        );
    }

    #[test]
    fn substitute_keys_and_values_in_object() {
        let config = json!({ "${workspaceRootFolderName}": "${lineNumber}", "hey ${env:key1} ": "${workspaceRootFolderName}" });
        let resolved = resolve_value(&config, &context(true), &no_config).unwrap();
        assert_eq!(
            resolved,
            json!({ "workspaceLocation": "10", "hey Value for key1 ": "workspaceLocation" })
        );
    }

    #[test]
    fn substitute_one_env_variable_using_platform_case_sensitivity() {
        assert_eq!(
            run("${env:key1} - ${env:Key1}", true).unwrap(),
            "Value for key1 - "
        );
    }

    fn font_settings() -> impl Fn(&str) -> Option<Value> {
        settings(
            json!({ "editor": { "fontFamily": "foo" }, "terminal": { "integrated": { "fontFamily": "bar" } } }),
        )
    }

    #[test]
    fn substitute_one_configuration_variable() {
        assert_eq!(
            resolve(
                "abc ${config:editor.fontFamily} xyz",
                &context(true),
                &font_settings()
            )
            .unwrap(),
            "abc foo xyz"
        );
    }

    #[test]
    fn inlines_an_array_245718() {
        let config = settings(json!({ "editor": { "fontFamily": ["foo", "bar"] } }));
        assert_eq!(
            resolve(
                "abc ${config:editor.fontFamily} xyz",
                &context(true),
                &config
            )
            .unwrap(),
            "abc foo,bar xyz"
        );
    }

    #[test]
    fn substitute_configuration_variable_with_undefined_workspace_folder() {
        assert_eq!(
            resolve(
                "abc ${config:editor.fontFamily} xyz",
                &context(false),
                &font_settings()
            )
            .unwrap(),
            "abc foo xyz"
        );
    }

    #[test]
    fn substitute_many_configuration_variables() {
        let text = "abc ${config:editor.fontFamily} ${config:terminal.integrated.fontFamily} xyz";
        assert_eq!(
            resolve(text, &context(true), &font_settings()).unwrap(),
            "abc foo bar xyz"
        );
    }

    #[test]
    fn substitute_one_env_variable_and_a_configuration_variable() {
        let text = "abc ${config:editor.fontFamily} ${workspaceFolder} ${env:key1} xyz";
        assert_eq!(
            resolve(text, &context(true), &font_settings()).unwrap(),
            "abc foo /VSCode/workspaceLocation Value for key1 xyz"
        );
    }

    #[test]
    fn recursively_resolve_variables() {
        let config = settings(
            json!({ "key1": "key1=${config:key2}", "key2": "key2=${config:key3}", "key3": "we did it!" }),
        );
        assert_eq!(
            resolve("${config:key1}", &context(true), &config).unwrap(),
            "key1=key2=we did it!"
        );
    }

    #[test]
    fn substitute_many_env_variable_and_a_configuration_variable() {
        let text = "${config:editor.fontFamily} ${config:terminal.integrated.fontFamily} ${workspaceFolder} - ${workspaceFolder} ${env:key1} - ${env:key2}";
        assert_eq!(
            resolve(text, &context(true), &font_settings()).unwrap(),
            "foo bar /VSCode/workspaceLocation - /VSCode/workspaceLocation Value for key1 - Value for key2"
        );
    }

    #[test]
    fn mixed_types_of_configuration_variables() {
        let config = settings(json!({
            "editor": { "fontFamily": "foo", "lineNumbers": 123, "insertSpaces": false },
            "json": { "schemas": [{ "fileMatch": ["/myfile", "/myOtherfile"], "url": "schemaURL" }] }
        }));
        let text = "abc ${config:editor.fontFamily} ${config:editor.lineNumbers} ${config:editor.insertSpaces} xyz";
        assert_eq!(
            resolve(text, &context(true), &config).unwrap(),
            "abc foo 123 false xyz"
        );
    }

    #[test]
    fn uses_original_variable_as_fallback() {
        assert_eq!(
            run("abc ${unknownVariable} xyz", true).unwrap(),
            "abc ${unknownVariable} xyz"
        );
        assert_eq!(
            run("abc ${env:unknownVariable} xyz", true).unwrap(),
            "abc  xyz"
        );
        assert_eq!(
            run("${command:foo} ${input:bar}", true).unwrap(),
            "${command:foo} ${input:bar}"
        );
    }

    #[test]
    fn configuration_variables_with_invalid_accessor() {
        let config = font_settings();
        let error = |text: &str| resolve(text, &context(true), &config).unwrap_err();
        assert_eq!(
            error("abc ${env} xyz"),
            "Variable ${env} can not be resolved because no environment variable name is given."
        );
        assert_eq!(
            error("abc ${env:} xyz"),
            "Variable ${env:} can not be resolved because no environment variable name is given."
        );
        assert_eq!(
            error("abc ${config} xyz"),
            "Variable ${config} can not be resolved because no settings name is given."
        );
        assert_eq!(
            error("abc ${config:} xyz"),
            "Variable ${config:} can not be resolved because no settings name is given."
        );
        assert_eq!(
            error("abc ${config:editor} xyz"),
            "Variable ${config:editor} can not be resolved because 'editor' is a structured value."
        );
        assert_eq!(
            error("abc ${config:editor..fontFamily} xyz"),
            "Variable ${config:editor..fontFamily} can not be resolved because setting 'editor..fontFamily' not found."
        );
        assert!(
            resolve(
                "abc ${config:editor.none.none2} xyz",
                &context(true),
                &config
            )
            .is_err()
        );
    }

    #[test]
    fn resolve_with_environment() {
        let mut context = context(false);
        context.env = HashMap::from([
            ("VAR_1".to_string(), "VAL_1".to_string()),
            ("VAR_2".to_string(), "VAL_2".to_string()),
        ]);
        assert_eq!(
            resolve("echo ${env:VAR_1}${env:VAR_2}", &context, &no_config).unwrap(),
            "echo VAL_1VAL_2"
        );
    }

    #[test]
    fn substitution_in_object_key() {
        let config = json!({ "name": "Test", "mappings": { "pos1": "value1", "${workspaceFolder}/test1": "${workspaceFolder}/test2", "pos3": "value3" } });
        let resolved = resolve_value(&config, &context(true), &no_config).unwrap();
        assert_eq!(
            resolved,
            json!({ "name": "Test", "mappings": { "pos1": "value1", "/VSCode/workspaceLocation/test1": "/VSCode/workspaceLocation/test2", "pos3": "value3" } })
        );
    }

    #[test]
    fn handles_root_string_value_with_multiple_variables() {
        let mut context = context(true);
        context.env = HashMap::from([
            ("HOME".to_string(), "/home/user".to_string()),
            ("SHELL".to_string(), "/bin/bash".to_string()),
        ]);
        assert_eq!(
            resolve("${env:HOME}/folder${env:SHELL}", &context, &no_config).unwrap(),
            "/home/user/folder/bin/bash"
        );
    }

    #[test]
    fn resolves_nested_values() {
        let mut context = context(true);
        context.env = HashMap::from([
            (
                "REDIRECTED".to_string(),
                "username: ${env:USERNAME}".to_string(),
            ),
            ("USERNAME".to_string(), "testuser".to_string()),
        ]);
        let config =
            json!({ "name": "${env:REDIRECTED}", "key that is ${env:REDIRECTED}": "cool!" });
        let resolved = resolve_value(&config, &context, &no_config).unwrap();
        assert_eq!(
            resolved,
            json!({ "name": "username: testuser", "key that is username: testuser": "cool!" })
        );
    }

    #[test]
    fn resolves_nested_values_2_245798() {
        let mut context = context(true);
        context.env = HashMap::from([
            ("site".to_string(), "example".to_string()),
            ("tld".to_string(), "com".to_string()),
            (
                "host".to_string(),
                "local.${env:site}.${env:tld}".to_string(),
            ),
        ]);
        let config =
            json!({ "env": { "SITE": "${env:site}", "TLD": "${env:tld}", "HOST": "${env:host}" } });
        let resolved = resolve_value(&config, &context, &no_config).unwrap();
        assert_eq!(
            resolved,
            json!({ "env": { "SITE": "example", "TLD": "com", "HOST": "local.example.com" } })
        );
    }

    #[test]
    fn self_reference_is_not_resolved_again() {
        let mut context = context(true);
        context.env = HashMap::from([
            ("FOO".to_string(), "a${env:BAR}".to_string()),
            ("BAR".to_string(), "b${env:FOO}".to_string()),
        ]);
        assert_eq!(
            resolve("${env:FOO}", &context, &no_config).unwrap(),
            "ab${env:FOO}"
        );
    }

    #[test]
    fn file_variables() {
        let mut context = context(true);
        context.file = Some(PathBuf::from("/VSCode/workspaceLocation/src/app.test.ts"));
        context.selected_text = Some("hello".to_string());
        context.home = Some(PathBuf::from("/Users/me"));
        context.cwd = PathBuf::from("/tmp");
        let cases = [
            ("${file}", "/VSCode/workspaceLocation/src/app.test.ts"),
            ("${fileWorkspaceFolder}", "/VSCode/workspaceLocation"),
            ("${fileWorkspaceFolderBasename}", "workspaceLocation"),
            ("${relativeFile}", "src/app.test.ts"),
            ("${relativeFileDirname}", "src"),
            ("${fileBasename}", "app.test.ts"),
            ("${fileBasenameNoExtension}", "app.test"),
            ("${fileDirname}", "/VSCode/workspaceLocation/src"),
            ("${fileDirnameBasename}", "src"),
            ("${fileExtname}", ".ts"),
            ("${columnNumber}", "10"),
            ("${selectedText}", "hello"),
            ("${userHome}", "/Users/me"),
            ("${cwd}", "/VSCode/workspaceLocation"),
            ("${pathSeparator}${/}", "//"),
            ("${execPath}", "${execPath}"),
        ];
        for (text, expected) in cases {
            assert_eq!(
                resolve(text, &context, &no_config).unwrap(),
                expected,
                "{text}"
            );
        }
        context.folder = None;
        assert_eq!(resolve("${cwd}", &context, &no_config).unwrap(), "/tmp");
        context.file = Some(PathBuf::from("/VSCode/workspaceLocation/file"));
        assert_eq!(
            resolve(
                "${relativeFileDirname:workspaceLocation}",
                &context,
                &no_config
            )
            .unwrap(),
            "."
        );
        context.file = Some(PathBuf::from("/elsewhere/file"));
        assert_eq!(
            resolve("${fileWorkspaceFolder}", &context, &no_config).unwrap_err(),
            "Variable ${fileWorkspaceFolder}: can not find workspace folder of 'file'."
        );
        context.file = None;
        assert_eq!(
            resolve("${fileBasename}", &context, &no_config).unwrap_err(),
            "Variable ${fileBasename} can not be resolved. Please open an editor."
        );
    }
}
