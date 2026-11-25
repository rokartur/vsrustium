//! `ImplicitActivationEvents` (implicitActivationEvents.ts): the activation events an extension gets from its
//! contributions, which the renderer adds to `activationEvents` before the init data reaches the host.

use serde_json::Value;

/// The `activationEventsGenerator` of each extension point registered in the desktop workbench, without the chat,
/// language model and MCP points (no AI by decision).
const GENERATORS: &[(&str, fn(&Value, &mut Vec<String>))] = &[
    ("authentication", |contrib, events| prefixed(events, "onAuthenticationRequest", contrib.get("id"))),
    ("commands", |contrib, events| prefixed(events, "onCommand", contrib.get("command"))),
    ("linkPresentationProviders", |contrib, events| prefixed(events, "onLinkPresentation", contrib.get("id"))),
    ("languages", |contrib, events| {
        if contrib.get("configuration").is_some_and(truthy) {
            prefixed(events, "onLanguage", contrib.get("id"));
        }
    }),
    ("views", |contrib, events| {
        for descriptors in contrib.as_object().into_iter().flat_map(|locations| locations.values()) {
            for descriptor in descriptors.as_array().into_iter().flatten() {
                prefixed(events, "onView", descriptor.get("id"));
            }
        }
    }),
    ("taskDefinitions", |contrib, events| prefixed(events, "onTaskType", contrib.get("type"))),
    ("debugVisualizers", |contrib, events| prefixed(events, "onDebugVisualizer", contrib.get("id"))),
    ("customEditors", |contrib, events| prefixed(events, "onCustomEditor", contrib.get("viewType"))),
    ("walkthroughs", |contrib, events| prefixed(events, "onWalkthrough", contrib.get("id"))),
    // terminalQuickFixService.ts and terminal.ts yield without checking the id.
    ("terminalQuickFixes", |contrib, events| events.push(format!("onTerminalQuickFixRequest:{}", js_string(contrib.get("id"))))),
    ("terminal", |contrib, events| {
        for profile in contrib.get("profiles").and_then(Value::as_array).into_iter().flatten() {
            events.push(format!("onTerminalProfile:{}", js_string(profile.get("id"))));
        }
    }),
    ("notebooks", |contrib, events| prefixed(events, "onNotebookSerializer", contrib.get("type"))),
    ("notebookRenderer", |contrib, events| prefixed(events, "onRenderer", contrib.get("id"))),
];

/// `readActivationEvents`: the declared events (`onUri` scoped to the extension) and the implicit ones, empty for an
/// extension without code.
pub fn read_activation_events(id: &str, description: &Value) -> Vec<String> {
    if description.get("main").is_none() && description.get("browser").is_none() {
        return Vec::new();
    }
    let mut events: Vec<String> = description["activationEvents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|event| match js_string(Some(event)) {
            event if event == "onUri" => format!("onUri:{}", id.to_lowercase()),
            event => event,
        })
        .collect();
    let Some(contributes) = description.get("contributes").and_then(Value::as_object) else { return events };
    for (point, contrib) in contributes {
        let Some((_, generator)) = GENERATORS.iter().find(|(name, _)| name == point) else { continue };
        match contrib {
            Value::Array(contribs) => contribs.iter().for_each(|contrib| generator(contrib, &mut events)),
            contrib => generator(contrib, &mut events),
        }
    }
    events
}

/// The `if (x) { yield \`prefix:${x}\` }` of most generators.
fn prefixed(events: &mut Vec<String>, prefix: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| truthy(value)) {
        events.push(format!("{prefix}:{}", js_string(Some(value))));
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// A value in a JavaScript template string.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(items)) => items.iter().map(|item| if item.is_null() { String::new() } else { js_string(Some(item)) }).collect::<Vec<_>>().join(","),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contributions_add_their_activation_events_after_the_declared_ones() {
        let description = json!({
            "main": "./out/extension",
            "activationEvents": ["onLanguage:markdown", "onUri"],
            "contributes": {
                "commands": [{ "command": "markdown.showPreview", "title": "Open Preview" }, { "title": "no command" }],
                "languages": [{ "id": "markdown", "configuration": "./language-configuration.json" }, { "id": "plain" }],
                "views": { "explorer": [{ "id": "npm", "name": "NPM Scripts" }] },
                "taskDefinitions": [{ "type": "npm" }],
                "customEditors": [{ "viewType": "imagePreview.previewEditor" }],
                "terminal": { "profiles": [{ "id": "p", "title": "P" }] },
                "chatParticipants": [{ "id": "left.out" }],
            },
        });
        assert_eq!(
            read_activation_events("Vscode.Markdown", &description),
            [
                "onLanguage:markdown",
                "onUri:vscode.markdown",
                "onCommand:markdown.showPreview",
                "onLanguage:markdown",
                "onView:npm",
                "onTaskType:npm",
                "onCustomEditor:imagePreview.previewEditor",
                "onTerminalProfile:p",
            ]
        );
    }

    #[test]
    fn an_extension_without_code_has_no_activation_events() {
        let description = json!({ "activationEvents": ["*"], "contributes": { "commands": [{ "command": "a" }] } });
        assert!(read_activation_events("a.b", &description).is_empty());
        assert_eq!(read_activation_events("a.b", &json!({ "browser": "./web", "contributes": { "commands": { "command": "a" } } })), ["onCommand:a"]);
    }
}
