//! `${name}` and `${name:arg}` variables as platform/configuration/common/configurationVariables.ts parses them,
//! the substitution of configurationResolverExpression.ts and the variables of variableResolver.ts
//! (`AbstractVariableResolverService.evaluateSingleVariable`). The interactive `${command:}` and `${input:}`
//! resolution lives with the workbench, which runs commands and quick input.

#[cfg(test)]
mod tests;

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// `IConfigurationVariable`: `id` is the whole `${...}`, `inner` what the braces hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub id: String,
    pub inner: String,
    pub name: String,
    pub arg: Option<String>,
}

/// `parseConfigurationVariable`: the variable starting at byte `start` and the byte index of its closing brace,
/// braces counted so `${env:a${env:b}}` is one variable.
pub fn parse_configuration_variable(value: &str, start: usize) -> Option<(Variable, usize)> {
    let bytes = value.as_bytes();
    if bytes.get(start) != Some(&b'$') || bytes.get(start + 1) != Some(&b'{') {
        return None;
    }
    let mut end = start + 2;
    let mut brace_count = 1;
    while end < bytes.len() {
        match bytes[end] {
            b'{' => brace_count += 1,
            b'}' => {
                brace_count -= 1;
                if brace_count == 0 {
                    break;
                }
            }
            _ => {}
        }
        end += 1;
    }
    if brace_count != 0 {
        return None;
    }
    let id = value[start..=end].to_string();
    let inner = value[start + 2..end].to_string();
    let (name, arg) = match inner.split_once(':') {
        Some((name, arg)) => (name.to_string(), Some(arg.to_string())),
        None => (inner.clone(), None),
    };
    Some((Variable { id, inner, name, arg }, end))
}

/// `hasConfigurationVariable`: whether a `${` has its closing brace, scanned from the end.
pub fn has_configuration_variable(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut unmatched_closing_braces = 0;
    for offset in (0..bytes.len()).rev() {
        if bytes[offset] == b'}' {
            unmatched_closing_braces += 1;
        } else if bytes[offset] == b'{' && unmatched_closing_braces > 0 {
            unmatched_closing_braces -= 1;
            if offset > 0 && bytes[offset - 1] == b'$' {
                return true;
            }
        }
    }
    false
}

/// `IResolvedValue`: `None` keeps the variable text; `input` is the `inputs` entry an `${input:}` came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedValue {
    pub value: Option<String>,
    pub input: Option<Value>,
}

impl From<String> for ResolvedValue {
    fn from(value: String) -> Self {
        ResolvedValue { value: Some(value), input: None }
    }
}

/// `ConfigurationResolverExpression`: a configuration (a string or any JSON) whose variables, in keys and string
/// values, are resolved one at a time. A resolved value is parsed again, so variables it holds are found too;
/// a variable inside its own value stays as text.
pub struct Expression {
    root: Value,
    order: Vec<Variable>,
    resolved: HashMap<String, ResolvedValue>,
}

impl Expression {
    /// `ConfigurationResolverExpression.parse`: copies `value`, applies the `osx` keys over the top level and
    /// drops `windows`, `osx` and `linux`.
    pub fn parse(value: &Value) -> Self {
        let mut root = value.clone();
        if let Value::Object(object) = &mut root {
            if let Some(Value::Object(platform)) = object.get("osx").cloned() {
                for (key, value) in platform {
                    object.insert(key, value);
                }
            }
            for key in ["windows", "osx", "linux"] {
                object.shift_remove(key);
            }
        }
        let mut expression = Expression { root, order: Vec::new(), resolved: HashMap::new() };
        expression.collect_variables();
        expression
    }

    /// The variables not resolved yet, in the order they were found.
    pub fn unresolved(&self) -> Vec<Variable> {
        self.order.iter().filter(|variable| !self.resolved.contains_key(&variable.id)).cloned().collect()
    }

    /// The resolved variables with their values, in the order they were found.
    pub fn resolved(&self) -> Vec<(Variable, ResolvedValue)> {
        self.order.iter().filter_map(|variable| Some((variable.clone(), self.resolved.get(&variable.id)?.clone()))).collect()
    }

    /// Records `variable`'s value; a variable the configuration does not hold is ignored.
    pub fn resolve(&mut self, variable: &Variable, value: impl Into<ResolvedValue>) {
        if !self.order.iter().any(|found| found.id == variable.id) {
            return;
        }
        self.resolved.insert(variable.id.clone(), value.into());
        self.collect_variables();
    }

    /// The configuration with every resolved variable substituted; unresolved ones are left as written.
    pub fn to_object(&self) -> Value {
        self.substitute_value(&self.root)
    }

    fn collect_variables(&mut self) {
        let mut found = Vec::new();
        collect_from_value(&self.to_object(), &mut found);
        for variable in found {
            if !self.order.iter().any(|known| known.id == variable.id) {
                self.order.push(variable);
            }
        }
    }

    fn substitute_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.substitute(text, &mut Vec::new())),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.substitute_value(item)).collect()),
            Value::Object(object) => {
                let mut substituted = Map::new();
                for (key, value) in object {
                    substituted.insert(self.substitute(key, &mut Vec::new()), self.substitute_value(value));
                }
                Value::Object(substituted)
            }
            other => other.clone(),
        }
    }

    /// `path` holds the variables being substituted, so a value that names its own variable keeps it as text.
    fn substitute(&self, text: &str, path: &mut Vec<String>) -> String {
        let mut out = String::new();
        let mut pos = 0;
        while let Some(found) = text[pos..].find("${") {
            let start = pos + found;
            let Some((variable, end)) = parse_configuration_variable(text, start) else {
                out.push_str(&text[pos..start + 2]);
                pos = start + 2;
                continue;
            };
            out.push_str(&text[pos..start]);
            match self.resolved.get(&variable.id).and_then(|resolved| resolved.value.as_deref()) {
                Some(value) if !path.contains(&variable.id) => {
                    path.push(variable.id);
                    out.push_str(&self.substitute(value, path));
                    path.pop();
                }
                _ => out.push_str(&variable.id),
            }
            pos = end + 1;
        }
        out.push_str(&text[pos..]);
        out
    }
}

fn collect_from_value(value: &Value, found: &mut Vec<Variable>) {
    match value {
        Value::String(text) => collect_from_string(text, found),
        Value::Array(items) => items.iter().for_each(|item| collect_from_value(item, found)),
        Value::Object(object) => {
            for (key, value) in object {
                collect_from_string(key, found);
                collect_from_value(value, found);
            }
        }
        _ => {}
    }
}

fn collect_from_string(text: &str, found: &mut Vec<Variable>) {
    let mut pos = 0;
    while let Some(offset) = text[pos..].find("${") {
        let start = pos + offset;
        match parse_configuration_variable(text, start) {
            Some((variable, end)) => {
                found.push(variable);
                pos = end + 1;
            }
            None => pos = start + 2,
        }
    }
}

/// `VariableError`: the variable kind (`VariableKind`) and the message shown for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableError {
    pub kind: &'static str,
    pub message: String,
}

impl std::fmt::Display for VariableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for VariableError {}

fn error(kind: &'static str, message: String) -> VariableError {
    VariableError { kind, message }
}

/// `IVariableResolveContext` as data: the workspace folders by name with their paths, the active editor's file,
/// its workspace folder, selection and position, and lookups for settings and installed extensions.
pub struct Context {
    pub folders: Vec<(String, String)>,
    pub config: Box<dyn Fn(Option<&str>, &str) -> Option<Value>>,
    pub extension_location: Box<dyn Fn(&str) -> Option<String>>,
    pub file_path: Option<String>,
    pub workspace_folder_for_file: Option<String>,
    pub selected_text: Option<String>,
    pub line_number: Option<String>,
    pub column_number: Option<String>,
    pub exec_path: Option<String>,
    pub app_root: Option<String>,
    pub env: HashMap<String, String>,
    pub user_home: Option<String>,
    pub cwd: String,
}

/// `evaluateSingleVariable` without a command value mapping: `folder` is the path of the folder the configuration
/// belongs to; `env` replaces the context's environment and drops the user home, as `resolveWithEnvironment`
/// does. `Ok(None)` leaves the variable as written; `${command:}` and `${input:}` resolve to their own text here.
pub fn evaluate(variable: &Variable, folder: Option<&str>, context: &Context, env: Option<&HashMap<String, String>>) -> Result<Option<String>, VariableError> {
    let id = &variable.id;
    // JavaScript's `if (argument)`: an empty argument counts as none.
    let argument = variable.arg.as_deref().filter(|argument| !argument.is_empty());
    let file_path = |kind: &'static str| -> Result<String, VariableError> {
        context.file_path.clone().ok_or_else(|| error(kind, format!("Variable {id} can not be resolved. Please open an editor.")))
    };
    let folder_path_for_file = |kind: &'static str| -> Result<String, VariableError> {
        let file = file_path(kind)?;
        context.workspace_folder_for_file.clone().ok_or_else(|| error(kind, format!("Variable {id}: can not find workspace folder of '{}'.", basename(&file))))
    };
    let folder_path = |kind: &'static str| -> Result<String, VariableError> {
        if let Some(argument) = argument {
            return match context.folders.iter().rev().find(|(name, _)| name == argument) {
                Some((_, path)) => Ok(path.clone()),
                None => Err(error(kind, format!("Variable {kind} can not be resolved. No such folder '{argument}'."))),
            };
        }
        if let Some(folder) = folder {
            return Ok(folder.to_string());
        }
        if context.folders.len() > 1 {
            return Err(error(kind, format!("Variable {kind} can not be resolved in a multi folder workspace. Scope this variable using ':' and a workspace folder name.")));
        }
        Err(error(kind, format!("Variable {kind} can not be resolved. Please open a folder.")))
    };
    let value = match variable.name.as_str() {
        "env" => {
            let Some(argument) = argument else {
                return Err(error("env", format!("Variable {id} can not be resolved because no environment variable name is given.")));
            };
            env.unwrap_or(&context.env).get(argument).cloned().unwrap_or_default()
        }
        "config" => {
            let Some(argument) = argument else {
                return Err(error("config", format!("Variable {id} can not be resolved because no settings name is given.")));
            };
            match (context.config)(folder, argument) {
                None | Some(Value::Null) => return Err(error("config", format!("Variable {id} can not be resolved because setting '{argument}' not found."))),
                Some(Value::Object(_)) => return Err(error("config", format!("Variable {id} can not be resolved because '{argument}' is a structured value."))),
                Some(value) => js_string(&value),
            }
        }
        "extensionInstallFolder" => {
            let Some(argument) = argument else {
                return Err(error("extensionInstallFolder", format!("Variable {id} can not be resolved because no extension name is given.")));
            };
            (context.extension_location)(argument)
                .ok_or_else(|| error("extensionInstallFolder", format!("Variable {id} can not be resolved because the extension {argument} is not installed.")))?
        }
        "workspaceRoot" | "workspaceFolder" => folder_path("workspaceFolder")?,
        "cwd" if folder.is_none() && argument.is_none() => context.cwd.clone(),
        "cwd" => folder_path("cwd")?,
        "workspaceRootFolderName" | "workspaceFolderBasename" => basename(&folder_path("workspaceFolderBasename")?).to_string(),
        "userHome" => context.user_home.clone().filter(|_| env.is_none()).ok_or_else(|| error("userHome", format!("Variable {id} can not be resolved. UserHome path is not defined")))?,
        "lineNumber" => context.line_number.clone().ok_or_else(|| error("lineNumber", format!("Variable {id} can not be resolved. Make sure to have a line selected in the active editor.")))?,
        "columnNumber" => context.column_number.clone().ok_or_else(|| error("columnNumber", format!("Variable {id} can not be resolved. Make sure to have a column selected in the active editor.")))?,
        "selectedText" => context.selected_text.clone().filter(|text| !text.is_empty()).ok_or_else(|| error("selectedText", format!("Variable {id} can not be resolved. Make sure to have some text selected in the active editor.")))?,
        "file" => file_path("file")?,
        "fileWorkspaceFolder" => folder_path_for_file("fileWorkspaceFolder")?,
        "fileWorkspaceFolderBasename" => basename(&folder_path_for_file("fileWorkspaceFolderBasename")?).to_string(),
        "relativeFile" if folder.is_some() || argument.is_some() => relative(&folder_path("relativeFile")?, &file_path("relativeFile")?),
        "relativeFile" => file_path("relativeFile")?,
        "relativeFileDirname" => {
            let dirname = dirname(&file_path("relativeFileDirname")?).to_string();
            match folder.is_some() || argument.is_some() {
                true => Some(relative(&folder_path("relativeFileDirname")?, &dirname)).filter(|relative| !relative.is_empty()).unwrap_or_else(|| ".".into()),
                false => dirname,
            }
        }
        "fileDirname" => dirname(&file_path("fileDirname")?).to_string(),
        "fileExtname" => extname(&file_path("fileExtname")?).to_string(),
        "fileBasename" => basename(&file_path("fileBasename")?).to_string(),
        "fileBasenameNoExtension" => {
            let file = file_path("fileBasenameNoExtension")?;
            let basename = basename(&file);
            basename[..basename.len() - extname(basename).len()].to_string()
        }
        "fileDirnameBasename" => basename(dirname(&file_path("fileDirnameBasename")?)).to_string(),
        "execPath" => context.exec_path.clone().unwrap_or_else(|| id.clone()),
        "execInstallFolder" => context.app_root.clone().unwrap_or_else(|| id.clone()),
        "pathSeparator" | "/" => "/".to_string(),
        _ => id.clone(),
    };
    Ok(Some(value))
}

/// `resolveAsync`: every variable `evaluate` can resolve, in the order they are found.
pub fn resolve(folder: Option<&str>, value: &Value, context: &Context) -> Result<Value, VariableError> {
    resolve_expression(folder, value, context, None)
}

/// `resolveWithEnvironment`: like `resolve`, with `env` as the environment and no user home.
pub fn resolve_with_environment(env: &HashMap<String, String>, folder: Option<&str>, value: &str, context: &Context) -> Result<String, VariableError> {
    let resolved = resolve_expression(folder, &Value::String(value.to_string()), context, Some(env))?;
    Ok(resolved.as_str().unwrap_or_default().to_string())
}

fn resolve_expression(folder: Option<&str>, value: &Value, context: &Context, env: Option<&HashMap<String, String>>) -> Result<Value, VariableError> {
    let mut expression = Expression::parse(value);
    let mut seen = HashSet::new();
    while let Some(variable) = expression.unresolved().into_iter().find(|variable| seen.insert(variable.id.clone())) {
        if let Some(value) = evaluate(&variable, folder, context, env)? {
            expression.resolve(&variable, value);
        }
    }
    Ok(expression.to_object())
}

/// JavaScript's `String(value)` for a setting value: arrays join with commas, `null` inside them is empty.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items.iter().map(|item| if item.is_null() { String::new() } else { js_string(item) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// Node's `path.posix.basename`.
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or_default()
}

/// Node's `path.posix.dirname`.
fn dirname(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/",
        Some(index) => &trimmed[..index],
        None if path.starts_with('/') => "/",
        None => ".",
    }
}

/// Node's `path.posix.extname`: from the last `.` of the basename, empty for a leading dot only.
fn extname(path: &str) -> &str {
    let basename = basename(path);
    match basename.rfind('.') {
        Some(index) if index > 0 => &basename[index..],
        _ => "",
    }
}

/// Node's `path.posix.relative` for absolute paths.
fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|part| !part.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|part| !part.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(from, to)| from == to).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    parts.join("/")
}
