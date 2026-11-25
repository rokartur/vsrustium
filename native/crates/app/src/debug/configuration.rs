//! Debuggers contributed by extensions (debugger.ts `Debugger` and its `merge`), the launch configurations of
//! launch.json (debugConfigurationManager.ts `Launch`), and the `${command:...}` and `${input:...}` references
//! configurationResolver.ts resolves interactively (workbench/configuration_resolver.rs).

use serde_json::{Value, json};

use exthost::scan::ScannedExtension;

/// A debug type with its merged contribution.
#[derive(Clone, Debug, PartialEq)]
pub struct Debugger {
    pub debug_type: String,
    pub contribution: Value,
    /// The extension of the main contribution (`label`, `program` or `runtime`).
    pub extension_id: String,
}

impl Debugger {
    pub fn label(&self) -> &str {
        self.contribution["label"].as_str().unwrap_or(&self.debug_type)
    }

    /// `interestedInLanguage`.
    pub fn interested_in_language(&self, language_id: &str) -> bool {
        self.contribution["languages"].as_array().is_some_and(|languages| languages.iter().any(|language| language == language_id))
    }

    pub fn when(&self) -> Option<&str> {
        self.contribution["when"].as_str()
    }

    pub fn hidden_when(&self) -> Option<&str> {
        self.contribution["hiddenWhen"].as_str()
    }

    pub fn has_initial_configuration(&self) -> bool {
        !self.contribution["initialConfigurations"].is_null()
    }
}

/// debugger.ts `isDebuggerMainContribution`.
fn is_main_contribution(contribution: &Value) -> bool {
    contribution["type"].is_string() && (contribution["label"].is_string() || !contribution["program"].is_null() || !contribution["runtime"].is_null())
}

/// The debuggers of `contributes.debuggers`, merged by type as the adapter manager does: built-in contributions
/// keep their values, later ones only add keys.
pub fn debuggers(extensions: &[ScannedExtension]) -> Vec<Debugger> {
    let mut debuggers: Vec<Debugger> = Vec::new();
    for extension in extensions {
        let builtin = extension.description["isBuiltin"] == true;
        for contribution in extension.description["contributes"]["debuggers"].as_array().into_iter().flatten() {
            let Some(debug_type) = contribution["type"].as_str() else { continue };
            let index = match debuggers.iter().position(|debugger| debugger.debug_type == debug_type) {
                Some(index) => index,
                None => {
                    debuggers.push(Debugger { debug_type: debug_type.to_owned(), contribution: json!({}), extension_id: extension.id.clone() });
                    debuggers.len() - 1
                }
            };
            let debugger = &mut debuggers[index];
            mixin(&mut debugger.contribution, contribution, builtin, 0);
            if is_main_contribution(contribution) {
                debugger.extension_id = extension.id.clone();
            }
        }
    }
    debuggers
}

/// debugger.ts `mixin`: objects merge deeply, other values overwrite only with `overwrite`; `type` stays.
fn mixin(destination: &mut Value, source: &Value, overwrite: bool, level: usize) {
    let (Value::Object(destination), Value::Object(source)) = (destination, source) else { return };
    for (key, value) in source {
        match destination.get_mut(key) {
            Some(existing) if existing.is_object() && value.is_object() => mixin(existing, value, overwrite, level + 1),
            Some(existing) => {
                if overwrite && !(level == 0 && key == "type") {
                    *existing = value.clone();
                }
            }
            None => {
                destination.insert(key.clone(), value.clone());
            }
        }
    }
}

/// A launch.json of a folder: its `configurations` and `compounds`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub configurations: Vec<Value>,
    pub compounds: Vec<Value>,
    pub inputs: Vec<Value>,
}

impl Launch {
    /// The `launch` section as `Launch.getConfig` reads it; anything malformed counts as missing.
    pub fn from_section(section: Option<&Value>) -> Launch {
        let list = |key: &str| section.and_then(|section| section[key].as_array()).cloned().unwrap_or_default();
        let named = |value: &Value| value["name"].is_string();
        Launch { configurations: list("configurations").into_iter().filter(named).collect(), compounds: list("compounds").into_iter().filter(named).collect(), inputs: list("inputs") }
    }

    /// `getConfiguration(name)`: the first configuration with that name, a copy.
    pub fn configuration(&self, name: &str) -> Option<Value> {
        self.configurations.iter().find(|configuration| configuration["name"] == name).cloned()
    }

    pub fn compound(&self, name: &str) -> Option<&Value> {
        self.compounds.iter().find(|compound| compound["name"] == name)
    }

    /// `getConfigurationNames`: configurations not `presentation.hidden`, sorted as debugUtils.ts `getVisibleAndSorted`,
    /// then the compounds.
    pub fn configuration_names(&self) -> Vec<String> {
        let mut visible: Vec<&Value> = self.configurations.iter().chain(&self.compounds).filter(|config| config["presentation"]["hidden"] != true).collect();
        visible.sort_by(|first, second| compare_presentation(&first["presentation"], &second["presentation"]));
        visible.iter().filter_map(|config| config["name"].as_str().map(str::to_owned)).collect()
    }
}

/// debugUtils.ts `getVisibleAndSorted` comparison: entries with a presentation first, then by group and order.
fn compare_presentation(first: &Value, second: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (first.is_object(), second.is_object()) {
        (false, false) => return Ordering::Equal,
        (false, true) => return Ordering::Greater,
        (true, false) => return Ordering::Less,
        _ => {}
    }
    match (first["group"].as_str(), second["group"].as_str()) {
        (None, Some(_)) => return Ordering::Greater,
        (Some(_), None) => return Ordering::Less,
        (Some(a), Some(b)) if a != b => return a.cmp(b),
        _ => {}
    }
    match (first["order"].as_f64(), second["order"].as_f64()) {
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        (None, None) => Ordering::Equal,
    }
}

/// A `${command:...}` or `${input:...}` the configuration holds, in the order they appear.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Interactive {
    Command(String),
    Input(String),
}

impl Interactive {
    pub fn variable(&self) -> String {
        match self {
            Interactive::Command(name) => format!("${{command:{name}}}"),
            Interactive::Input(name) => format!("${{input:{name}}}"),
        }
    }
}

/// configurationResolver.ts `findVariables`: every interactive variable in the configuration's strings, once.
pub fn interactive_variables(config: &Value) -> Vec<Interactive> {
    let mut found = Vec::new();
    collect_variables(config, &mut found);
    found
}

fn collect_variables(value: &Value, found: &mut Vec<Interactive>) {
    match value {
        Value::String(text) => {
            let mut rest = text.as_str();
            while let Some(start) = rest.find("${") {
                let Some(end) = rest[start..].find('}') else { break };
                let inner = &rest[start + 2..start + end];
                let variable = match inner.split_once(':') {
                    Some(("command", name)) => Some(Interactive::Command(name.to_owned())),
                    Some(("input", name)) => Some(Interactive::Input(name.to_owned())),
                    _ => None,
                };
                if let Some(variable) = variable.filter(|variable| !found.contains(variable)) {
                    found.push(variable);
                }
                rest = &rest[start + end + 1..];
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_variables(item, found)),
        Value::Object(members) => members.values().for_each(|member| collect_variables(member, found)),
        _ => {}
    }
}

/// Replaces each resolved variable in every string; a string that is exactly one variable takes the value as it
/// is (a command may answer a non-string, which the resolver refuses before this).
pub fn replace_variables(value: &mut Value, resolved: &[(String, String)]) {
    match value {
        Value::String(text) => {
            for (variable, replacement) in resolved {
                if text.contains(variable.as_str()) {
                    *text = text.replace(variable.as_str(), replacement);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| replace_variables(item, resolved)),
        Value::Object(members) => members.values_mut().for_each(|member| replace_variables(member, resolved)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn interactive_variables_are_found_once_in_order() {
        let config = json!({ "program": "${workspaceFolder}/${input:file}", "args": ["${command:pickProcess}", "${input:file}"], "env": { "A": "${env:HOME}" } });
        assert_eq!(interactive_variables(&config), vec![Interactive::Input("file".into()), Interactive::Command("pickProcess".into())]);
    }

    #[test]
    fn replaced_variables_reach_nested_strings() {
        let mut config = json!({ "program": "${input:file}.js", "args": ["--pid=${command:pick}"] });
        replace_variables(&mut config, &[("${input:file}".into(), "main".into()), ("${command:pick}".into(), "42".into())]);
        assert_eq!(config, json!({ "program": "main.js", "args": ["--pid=42"] }));
    }

    #[test]
    fn debuggers_merge_by_type_and_keep_the_built_in_values() {
        let extension = |id: &str, builtin: bool, debuggers: Value| ScannedExtension {
            id: id.into(),
            location: std::path::PathBuf::from("/x"),
            description: json!({ "isBuiltin": builtin, "contributes": { "debuggers": debuggers } }),
        };
        let extensions = vec![
            extension("a.main", true, json!([{ "type": "node", "label": "Node.js", "languages": ["javascript"] }])),
            extension("b.extra", false, json!([{ "type": "node", "label": "Other", "variables": { "PickProcess": "extension.pickNodeProcess" } }])),
        ];
        let found = debuggers(&extensions);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label(), "Node.js");
        assert!(found[0].interested_in_language("javascript"));
        assert_eq!(found[0].contribution["variables"]["PickProcess"], "extension.pickNodeProcess");
        assert_eq!(found[0].extension_id, "b.extra");
    }

    #[test]
    fn configuration_names_put_presentation_groups_first() {
        let launch = Launch::from_section(Some(&json!({
            "configurations": [{ "name": "plain" }, { "name": "b", "presentation": { "group": "2" } }, { "name": "a", "presentation": { "group": "1", "order": 2 } }, { "name": "hidden", "presentation": { "hidden": true } }],
            "compounds": [{ "name": "both", "configurations": ["a", "b"] }],
        })));
        assert_eq!(launch.configuration_names(), vec!["a", "b", "plain", "both"]);
        assert!(launch.configuration("missing").is_none());
    }
}
