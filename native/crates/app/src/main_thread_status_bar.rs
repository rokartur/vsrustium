//! The main-thread side of `window.createStatusBarItem`: workbench/api/browser/mainThreadStatusBar.ts and the
//! `statusBarItems` contribution of statusBarExtensionPoint.ts.

use exthost::scan::ScannedExtension;
use gpui::Context;
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::workbench::Workbench;

/// An `IStatusbarEntry` from an extension, kept in statusbarModel.ts order.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusEntry {
    pub entry_id: String,
    pub id: String,
    pub name: String,
    pub text: String,
    pub tooltip: Option<String>,
    /// A command id, or a `Command` object with `arguments`.
    pub command: Option<Value>,
    pub color: Option<Value>,
    pub background_color: Option<Value>,
    /// `statusBarItem.errorBackground` and `warningBackground` become the entry's kind (statusBarExtensionPoint.ts).
    pub kind: Option<&'static str>,
    pub align_left: bool,
    pub priority: f64,
    pub secondary: i32,
    pub show_progress: bool,
}

/// `ExtensionStatusBarItemService.setOrUpdateEntry`; alignment and priority stay those of the first call.
pub fn set_or_update_entry(entries: &mut Vec<StatusEntry>, mut entry: StatusEntry) {
    if let Some(background) = entry.background_color.as_ref().and_then(|color| color["id"].as_str()) {
        let kind = match background {
            "statusBarItem.errorBackground" => Some("error"),
            "statusBarItem.warningBackground" => Some("warning"),
            _ => None,
        };
        if kind.is_some() {
            entry.kind = kind;
            entry.color = None;
            entry.background_color = None;
        }
    }
    if let Some(existing) = entries.iter_mut().find(|existing| existing.entry_id == entry.entry_id) {
        entry.align_left = existing.align_left;
        entry.priority = existing.priority;
        entry.secondary = existing.secondary;
        *existing = entry;
        return;
    }
    // statusbarModel.ts `sort`: higher priority to the left, then the extension id hash, then insertion order.
    let index = entries
        .iter()
        .position(|other| other.align_left == entry.align_left && (other.priority < entry.priority || other.priority == entry.priority && other.secondary < entry.secondary))
        .unwrap_or(entries.len());
    entries.insert(index, entry);
}

/// `$setEntry(entryId, id, extensionId, name, text, tooltip, hasTooltipProvider, command, color, backgroundColor,
/// alignLeft, priority, accessibilityInformation)`.
fn entry_from_args(args: &[Value]) -> StatusEntry {
    let extension_id = args[2].as_str();
    let tooltip = match &args[5] {
        Value::String(text) => Some(text.clone()),
        Value::Object(markdown) => markdown.get("value").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    };
    let command = match &args[7] {
        Value::Null => None,
        command => Some(json!({ "id": command["id"], "arguments": command["arguments"] })),
    };
    StatusEntry {
        entry_id: args[0].as_str().unwrap_or_default().to_owned(),
        id: args[1].as_str().unwrap_or_default().to_owned(),
        name: args[3].as_str().unwrap_or_default().to_owned(),
        text: args[4].as_str().unwrap_or_default().to_owned(),
        tooltip,
        command,
        color: Some(args[8].clone()).filter(|color| !color.is_null()),
        background_color: Some(args[9].clone()).filter(|color| !color.is_null()),
        kind: None,
        align_left: args[10] == true,
        priority: args[11].as_f64().unwrap_or(0.),
        secondary: extension_id.map_or(0, string_hash),
        show_progress: false,
    }
}

/// base/common/hash.ts `hash(string)`.
pub(crate) fn string_hash(text: &str) -> i32 {
    let number_hash = |value: i32, initial: i32| initial.wrapping_shl(5).wrapping_sub(initial).wrapping_add(value);
    let mut hash = number_hash(149417, 0);
    for unit in text.encode_utf16() {
        hash = number_hash(i32::from(unit), hash);
    }
    hash
}

/// statusBarExtensionPoint.ts: the `statusBarItems` of extensions with the `contribStatusBarItems` proposal, in
/// mainThreadStatusBar.ts's `asDto` shape for `$acceptStaticEntries`.
pub fn static_entries(extensions: &[ScannedExtension]) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    for extension in extensions {
        let proposals = extension.description["enabledApiProposals"].as_array();
        if !proposals.is_some_and(|proposals| proposals.iter().any(|proposal| proposal.as_str().is_some_and(|name| name.split('@').next() == Some("contribStatusBarItems")))) {
            continue;
        }
        let items = match &extension.description["contributes"]["statusBarItems"] {
            Value::Array(items) => items.clone(),
            Value::Null => Vec::new(),
            item => vec![item.clone()],
        };
        let key = extension.id.to_lowercase();
        for item in items.iter().filter(|item| is_user_friendly_entry(item)) {
            let entry_id = format!("{key}.{}", item["id"].as_str().unwrap_or_default());
            let entry = StatusEntry {
                entry_id: entry_id.clone(),
                id: entry_id,
                name: item["name"].as_str().unwrap_or_default().to_owned(),
                text: item["text"].as_str().unwrap_or_default().to_owned(),
                tooltip: item["tooltip"].as_str().map(str::to_owned),
                command: item["command"].as_str().map(|id| json!(id)),
                color: None,
                background_color: None,
                kind: None,
                align_left: item["alignment"] == "left",
                priority: item["priority"].as_f64().unwrap_or(0.),
                secondary: string_hash(&key),
                show_progress: false,
            };
            set_or_update_entry(&mut entries, entry);
        }
    }
    entries
}

/// `isUserFriendlyStatusItemEntry`.
fn is_user_friendly_entry(item: &Value) -> bool {
    item["id"].as_str().is_some_and(|id| !id.is_empty())
        && item["name"].is_string()
        && item["text"].is_string()
        && (item["alignment"] == "left" || item["alignment"] == "right")
        && (item["command"].is_null() || item["command"].is_string())
        && (item["tooltip"].is_null() || item["tooltip"].is_string())
        && (item["priority"].is_null() || item["priority"].is_number())
        && (item["accessibilityInformation"].is_null() || item["accessibilityInformation"]["label"].is_string())
}

/// mainThreadStatusBar.ts `asDto`, with the static entry's accessibility information from its contribution.
pub fn static_entry_dtos(extensions: &[ScannedExtension]) -> Vec<Value> {
    let accessibility = |entry_id: &str| {
        extensions.iter().find_map(|extension| {
            let items = &extension.description["contributes"]["statusBarItems"];
            let items = items.as_array().cloned().unwrap_or_else(|| vec![items.clone()]);
            items.into_iter().find(|item| format!("{}.{}", extension.id.to_lowercase(), item["id"].as_str().unwrap_or_default()) == entry_id).map(|item| item["accessibilityInformation"].clone())
        })
    };
    static_entries(extensions)
        .into_iter()
        .map(|entry| {
            let mut dto = json!({ "entryId": entry.entry_id, "name": entry.name, "text": entry.text, "priority": entry.priority, "alignLeft": entry.align_left });
            if let Some(tooltip) = entry.tooltip {
                dto["tooltip"] = json!(tooltip);
            }
            if let Some(command) = entry.command {
                dto["command"] = command;
            }
            if let Some(info) = accessibility(&dto["entryId"].as_str().unwrap_or_default().to_owned()).filter(|info| !info.is_null()) {
                dto["accessibilityInformation"] = json!({ "label": info["label"], "role": info["role"] });
            }
            dto
        })
        .collect()
}

impl Workbench {
    /// `$setEntry`: defines or updates the entry; `$disposeEntry` removes it.
    pub(crate) fn set_extension_status_entry(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        set_or_update_entry(&mut self.status_entries, entry_from_args(args));
        cx.notify();
        Outcome::done()
    }

    pub(crate) fn dispose_extension_status_entry(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        self.status_entries.retain(|entry| Some(entry.entry_id.as_str()) != args[0].as_str());
        cx.notify();
        Outcome::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(entry_id: &str, align_left: bool, priority: f64) -> StatusEntry {
        entry_from_args(&[json!(entry_id), json!(entry_id), Value::Null, json!(""), json!(entry_id), Value::Null, json!(false), Value::Null, Value::Null, Value::Null, json!(align_left), json!(priority)])
    }

    #[test]
    fn higher_priority_sorts_left_and_updates_keep_the_first_place() {
        let mut entries = Vec::new();
        for (id, priority) in [("a", 1.), ("b", 5.), ("c", 1.), ("d", 3.)] {
            set_or_update_entry(&mut entries, entry(id, true, priority));
        }
        let ids = |entries: &[StatusEntry]| entries.iter().map(|entry| entry.entry_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&entries), ["b", "d", "a", "c"]);
        let mut moved = entry("a", false, 100.);
        moved.text = "new".into();
        set_or_update_entry(&mut entries, moved);
        assert_eq!(ids(&entries), ["b", "d", "a", "c"]);
        assert_eq!((entries[2].text.as_str(), entries[2].align_left, entries[2].priority), ("new", true, 1.));
    }

    #[test]
    fn error_and_warning_backgrounds_become_the_kind() {
        let mut entries = Vec::new();
        let mut warning = entry("w", true, 0.);
        warning.background_color = Some(json!({ "id": "statusBarItem.warningBackground" }));
        warning.color = Some(json!("#ff0000"));
        set_or_update_entry(&mut entries, warning);
        assert_eq!((entries[0].kind, &entries[0].color, &entries[0].background_color), (Some("warning"), &None, &None));
    }

    #[test]
    fn hash_matches_base_common_hash() {
        assert_eq!(string_hash(""), 149417);
        assert_eq!(string_hash("a"), 149417 * 31 + 97);
    }

    #[test]
    fn static_entries_need_the_proposal() {
        let item = json!({ "id": "myStaticItem", "alignment": "right", "priority": 17, "name": "My Static Item", "text": "Hello $(globe)", "tooltip": "Hover World", "accessibilityInformation": { "label": "Hello World", "role": "button" } });
        let extension = |proposals: Value| ScannedExtension { id: "vscode.vscode-api-tests".into(), location: "/x".into(), description: json!({ "enabledApiProposals": proposals, "contributes": { "statusBarItems": item } }) };
        assert!(static_entry_dtos(&[extension(json!([]))]).is_empty());
        assert_eq!(
            static_entry_dtos(&[extension(json!(["contribStatusBarItems"]))]),
            [json!({ "entryId": "vscode.vscode-api-tests.myStaticItem", "name": "My Static Item", "text": "Hello $(globe)", "tooltip": "Hover World", "priority": 17.0, "alignLeft": false, "accessibilityInformation": { "label": "Hello World", "role": "button" } })]
        );
    }
}
