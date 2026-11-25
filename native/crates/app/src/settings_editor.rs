//! The Settings editor's model: contrib/preferences/browser/settingsLayout.ts (`tocData`), settingsTree.ts
//! (`resolveSettingsTree`, `createTocTreeForExtensionSettings`), settingsTreeModels.ts (the setting value types,
//! labels, filters and `parseQuery`) and preferencesSearch.ts (`LocalSearchProvider`, `SettingMatches`).

#[cfg(test)]
mod tests;

pub mod lists;
pub mod validation;

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;

use config::configuration::{Configuration, Overrides};
use config::model::ConfigurationModel;
use config::registry::{APPLICATION_SCOPES, FOLDER_SCOPES, Scope, WORKSPACE_SCOPES};
use config::service::Target;
use serde_json::Value;

use crate::command_palette::locale_compare;
use crate::default_settings::{Setting, SettingsGroup};
use crate::filters::{matches_base_contiguous_sub_string, matches_contiguous_sub_string, matches_contiguous_words, matches_sub_string, matches_words};

pub const MODIFIED_SETTING_TAG: &str = "modified";
pub const ADVANCED_SETTING_TAG: &str = "advanced";
const POLICY_SETTING_TAG: &str = "hasPolicy";
const REQUIRE_TRUSTED_WORKSPACE_SETTING_TAG: &str = "requireTrustedWorkspace";

/// `ITOCEntry<string>` of settingsLayout.ts.
struct TocNode {
    id: &'static str,
    label: &'static str,
    settings: &'static [&'static str],
    children: &'static [TocNode],
    hide: bool,
}

const fn leaf(id: &'static str, label: &'static str, settings: &'static [&'static str]) -> TocNode {
    TocNode { id, label, settings, children: &[], hide: false }
}

/// `COMMONLY_USED_SETTINGS`.
const COMMONLY_USED_SETTINGS: &[&str] = &[
    "editor.fontSize",
    "editor.formatOnSave",
    "files.autoSave",
    "GitHub.copilot-chat.manageExtension",
    "editor.defaultFormatter",
    "editor.fontFamily",
    "editor.wordWrap",
    "chat.agent.maxRequests",
    "files.exclude",
    "workbench.colorTheme",
    "editor.tabSize",
    "editor.mouseWheelZoom",
    "editor.formatOnPaste",
];

/// The settings that belong to more than one TOC group (`multiGroupTocSettings`).
const MULTI_GROUP_TOC_SETTINGS: &[&str] = &["accessibility.signals.chatUserActionRequired", "accessibility.signals.chatResponseReceived"];

/// `tocData`, with `isWeb` and `isWindows` false.
const TOC_DATA: &[TocNode] = &[
    TocNode {
        id: "editor",
        label: "Text Editor",
        settings: &["editor.*"],
        children: &[
            leaf("editor/cursor", "Cursor", &["editor.cursor*"]),
            leaf("editor/find", "Find", &["editor.find.*"]),
            leaf("editor/font", "Font", &["editor.font*"]),
            leaf("editor/format", "Formatting", &["editor.format*"]),
            leaf("editor/diffEditor", "Diff Editor", &["diffEditor.*"]),
            leaf("editor/multiDiffEditor", "Multi-File Diff Editor", &["multiDiffEditor.*"]),
            leaf("editor/minimap", "Minimap", &["editor.minimap.*"]),
            leaf("editor/suggestions", "Suggestions", &["editor.*suggest*"]),
            leaf("editor/files", "Files", &["files.*"]),
        ],
        hide: false,
    },
    TocNode {
        id: "workbench",
        label: "Workbench",
        settings: &["workbench.*"],
        children: &[
            leaf(
                "workbench/appearance",
                "Appearance",
                &["workbench.activityBar.*", "workbench.*color*", "workbench.fontAliasing", "workbench.iconTheme", "workbench.sidebar.location", "workbench.*.visible", "workbench.tips.enabled", "workbench.tree.*", "workbench.view.*"],
            ),
            leaf("workbench/breadcrumbs", "Breadcrumbs", &["breadcrumbs.*"]),
            leaf("workbench/editor", "Editor Management", &["workbench.editor.*"]),
            leaf("workbench/settings", "Settings Editor", &["workbench.settings.*"]),
            leaf("workbench/zenmode", "Zen Mode", &["zenmode.*"]),
            leaf("workbench/screencastmode", "Screencast Mode", &["screencastMode.*"]),
            leaf("workbench/browser", "Browser", &["workbench.browser.*"]),
        ],
        hide: false,
    },
    TocNode { id: "window", label: "Window", settings: &["window.*"], children: &[leaf("window/newWindow", "New Window", &["window.*newwindow*"])], hide: false },
    TocNode {
        id: "chat",
        label: "Chat",
        settings: &[],
        children: &[
            leaf(
                "chat/agent",
                "Agent",
                &[
                    "chat.agent.*",
                    "chat.checkpoints.*",
                    "chat.editRequests",
                    "chat.requestQueuing.*",
                    "chat.undoRequests.*",
                    "chat.customAgentInSubagent.*",
                    "chat.editing.autoAcceptDelay",
                    "chat.editing.confirmEditRequest*",
                    "chat.planAgent.defaultModel",
                ],
            ),
            leaf(
                "chat/appearance",
                "Appearance",
                &[
                    "chat.editor.*",
                    "chat.fontFamily",
                    "chat.fontSize",
                    "chat.math.*",
                    "chat.agentsControl.*",
                    "chat.alternativeToolAction.*",
                    "chat.codeBlock.*",
                    "chat.editing.explainChanges.enabled",
                    "chat.editorAssociations",
                    "chat.extensionUnification.*",
                    "chat.inlineReferences.*",
                    "chat.notifyWindow*",
                    "chat.statusWidget.*",
                    "chat.tips.*",
                    "chat.unifiedAgentsBar.*",
                    "accessibility.signals.chatUserActionRequired",
                    "accessibility.signals.chatResponseReceived",
                ],
            ),
            leaf(
                "chat/sessions",
                "Sessions",
                &["chat.agentSessionProjection.*", "chat.sessions.*", "chat.viewProgressBadge.*", "chat.viewSessions.*", "chat.restoreLastPanelSession", "chat.exitAfterDelegation", "chat.repoInfo.*"],
            ),
            leaf("chat/tools", "Tools", &["chat.tools.*", "chat.extensionTools.*"]),
            leaf("chat/mcp", "MCP", &["mcp", "chat.mcp.*", "mcp.*"]),
            leaf(
                "chat/context",
                "Context",
                &[
                    "chat.detectParticipant.*",
                    "chat.experimental.detectParticipant.*",
                    "chat.implicitContext.*",
                    "chat.promptFilesLocations",
                    "chat.instructionsFilesLocations",
                    "chat.modeFilesLocations",
                    "chat.agentFilesLocations",
                    "chat.agentSkillsLocations",
                    "chat.hookFilesLocations",
                    "chat.promptFilesRecommendations",
                    "chat.useAgentsMdFile",
                    "chat.useNestedAgentsMdFiles",
                    "chat.useAgentSkills",
                    "chat.experimental.useSkillAdherencePrompt",
                    "chat.useHooks",
                    "chat.includeApplyingInstructions",
                    "chat.includeReferencedInstructions",
                    "chat.useClaudeMdFile",
                ],
            ),
            leaf("chat/inlineChat", "Inline Chat", &["inlineChat.*"]),
            leaf("chat/miscellaneous", "Miscellaneous", &["chat.disableAIFeatures", "chat.allowAnonymousAccess"]),
        ],
        hide: false,
    },
    TocNode {
        id: "features",
        label: "Features",
        settings: &[],
        children: &[
            leaf("features/accessibilitySignals", "Accessibility Signals", &["accessibility.signal*"]),
            leaf("features/accessibility", "Accessibility", &["accessibility.*"]),
            leaf("features/explorer", "Explorer", &["explorer.*", "outline.*"]),
            leaf("features/search", "Search", &["search.*"]),
            leaf("features/debug", "Debug", &["debug.*", "launch"]),
            leaf("features/testing", "Testing", &["testing.*"]),
            leaf("features/scm", "Source Control", &["scm.*"]),
            leaf("features/extensions", "Extensions", &["extensions.*"]),
            leaf("features/terminal", "Terminal", &["terminal.*"]),
            leaf("features/task", "Task", &["task.*"]),
            leaf("features/problems", "Problems", &["problems.*"]),
            leaf("features/output", "Output", &["output.*"]),
            leaf("features/comments", "Comments", &["comments.*"]),
            leaf("features/remote", "Remote", &["remote.*"]),
            leaf("features/timeline", "Timeline", &["timeline.*"]),
            leaf("features/notebook", "Notebook", &["notebook.*", "interactiveWindow.*"]),
            leaf("features/mergeEditor", "Merge Editor", &["mergeEditor.*"]),
            TocNode { id: "features/issueReporter", label: "Issue Reporter", settings: &["issueReporter.*"], children: &[], hide: true },
        ],
        hide: false,
    },
    TocNode {
        id: "application",
        label: "Application",
        settings: &[],
        children: &[
            leaf("application/http", "Proxy", &["http.*"]),
            leaf("application/keyboard", "Keyboard", &["keyboard.*"]),
            leaf("application/update", "Update", &["update.*"]),
            leaf("application/telemetry", "Telemetry", &["telemetry.*"]),
            leaf("application/settingsSync", "Settings Sync", &["settingsSync.*"]),
            leaf("application/network", "Network", &["network.*"]),
            leaf("application/experimental", "Experimental", &["application.experimental.*"]),
            leaf("application/other", "Other", &["application.*"]),
        ],
        hide: false,
    },
    TocNode { id: "security", label: "Security", settings: &["security.*"], children: &[leaf("security/workspace", "Workspace", &["security.workspace.*"])], hide: false },
];

/// A resolved `ITOCEntry<ISetting>`: its settings are indexes into `Model::settings`.
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub settings: Vec<usize>,
    pub children: Vec<Group>,
}

/// The registered settings and the resolved settings tree, Commonly Used first and Extensions last.
#[derive(Default)]
pub struct Model {
    pub settings: Vec<Setting>,
    pub root: Group,
}

impl Model {
    /// `onConfigUpdate`: the tree of `groups`, advanced settings left out unless `show_advanced`.
    /// `extension_name` is the display name of an installed extension.
    pub fn new(groups: &[SettingsGroup], show_advanced: bool, extension_name: impl Fn(&str) -> String) -> Model {
        let mut settings = Vec::new();
        let mut group_indexes = Vec::new();
        for group in groups {
            let mut indexes = Vec::new();
            for setting in &group.settings {
                indexes.push(settings.len());
                settings.push(setting.clone());
            }
            group_indexes.push(indexes);
        }
        let exclude_advanced = !show_advanced;
        // `getFlatSettings` of the core groups: the settings not taken by a TOC entry yet.
        let mut left: Vec<usize> = Vec::new();
        for (group, indexes) in groups.iter().zip(&group_indexes) {
            if group.extension_id.is_none() {
                left.extend(indexes.iter().copied().filter(|&index| settings[index].overrides.is_empty()));
            }
        }
        let mut children = Vec::new();
        let commonly_used = COMMONLY_USED_SETTINGS.iter().filter_map(|key| settings.iter().position(|setting| setting.key == *key)).collect();
        children.push(Group { id: "commonlyUsed".into(), label: "Commonly Used".into(), settings: commonly_used, children: Vec::new() });
        for node in TOC_DATA.iter().filter(|node| !node.hide) {
            let group = resolve_node(node, &settings, &mut left, exclude_advanced);
            if !group.children.is_empty() || !group.settings.is_empty() {
                children.push(group);
            }
        }
        let extension_groups: Vec<(&SettingsGroup, &Vec<usize>)> = groups.iter().zip(&group_indexes).filter(|(group, _)| group.extension_id.is_some()).collect();
        children.push(extensions_group(&extension_groups, &settings, exclude_advanced, extension_name));
        Model { settings, root: Group { id: "root".into(), label: "root".into(), settings: Vec::new(), children } }
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.settings.iter().position(|setting| setting.key == key)
    }
}

/// `_resolveSettingsTree`: children take their settings first, the node gets what is left of its patterns.
fn resolve_node(node: &TocNode, settings: &[Setting], left: &mut Vec<usize>, exclude_advanced: bool) -> Group {
    let mut children = Vec::new();
    for child in node.children.iter().filter(|child| !child.hide) {
        let group = resolve_node(child, settings, left, exclude_advanced);
        if !group.children.is_empty() || !group.settings.is_empty() {
            children.push(group);
        }
    }
    let patterns: Vec<regex::Regex> = node.settings.iter().map(|pattern| setting_match_regex(pattern)).collect();
    let mut matched = Vec::new();
    left.retain(|&index| {
        let setting = &settings[index];
        let included = patterns.iter().any(|pattern| pattern.is_match(&setting.key)) && !(exclude_advanced && has_tag(setting, ADVANCED_SETTING_TAG));
        if included {
            matched.push(index);
        }
        !included || MULTI_GROUP_TOC_SETTINGS.contains(&setting.key.as_str())
    });
    sort_settings(&mut matched, settings);
    Group { id: node.id.into(), label: node.label.into(), settings: matched, children }
}

/// `createTocTreeForExtensionSettings`.
fn extensions_group(groups: &[(&SettingsGroup, &Vec<usize>)], settings: &[Setting], exclude_advanced: bool, extension_name: impl Fn(&str) -> String) -> Group {
    let mut by_extension: Vec<(String, Group, Vec<Option<f64>>)> = Vec::new();
    for (group, indexes) in groups {
        let extension_id = group.extension_id.clone().expect("an extension group");
        let mut matched: Vec<usize> = indexes.iter().copied().filter(|&index| !(exclude_advanced && has_tag(&settings[index], ADVANCED_SETTING_TAG))).collect();
        sort_settings(&mut matched, settings);
        let id = if !group.id.is_empty() && group.id != extension_id { group.id.clone() } else { group.title.clone() };
        let child = Group { id, label: group.title.clone(), settings: matched, children: Vec::new() };
        let index = match by_extension.iter().position(|(id, ..)| *id == extension_id) {
            Some(index) => index,
            None => {
                let label = extension_name(&extension_id);
                by_extension.push((extension_id.clone(), Group { id: extension_id, label, ..Group::default() }, Vec::new()));
                by_extension.len() - 1
            }
        };
        by_extension[index].1.children.push(child);
        by_extension[index].2.push(group.order);
    }
    let mut extension_groups = Vec::new();
    for (_, mut root, orders) in by_extension {
        if root.children.len() == 1 {
            let only = root.children.remove(0);
            extension_groups.push(Group { id: root.id, label: only.label, settings: only.settings, children: Vec::new() });
            continue;
        }
        let mut ordered: Vec<(Option<f64>, Group)> = orders.into_iter().zip(root.children).collect();
        ordered.sort_by(|a, b| compare_nullable_numbers(a.0, b.0));
        root.children = ordered.into_iter().map(|(_, group)| group).collect();
        // https://github.com/microsoft/vscode/issues/137259: a category titled as the extension holds its ungrouped settings.
        if let Some(ungrouped) = root.children.iter().position(|child| child.label == root.label && child.children.is_empty()) {
            let ungrouped = root.children.remove(ungrouped);
            root.settings = ungrouped.settings;
        }
        extension_groups.push(root);
    }
    extension_groups.sort_by(|a, b| locale_compare(&a.label, &b.label));
    Group { id: "extensions".into(), label: "Extensions".into(), settings: Vec::new(), children: extension_groups }
}

/// `createSettingMatchRegExp`: `*` matches anything, the rest literally, ignoring case.
fn setting_match_regex(pattern: &str) -> regex::Regex {
    let source = format!("^{}$", regex::escape(pattern).replace(r"\*", ".*"));
    regex::RegexBuilder::new(&source).case_insensitive(true).build().expect("an escaped pattern is valid")
}

/// `compareTwoNullableNumbers`: undefined after any number.
fn compare_nullable_numbers(a: Option<f64>, b: Option<f64>) -> Ordering {
    let max = 9007199254740991.;
    a.unwrap_or(max).partial_cmp(&b.unwrap_or(max)).unwrap_or(Ordering::Equal)
}

/// `sortSettings`: stable, then preview, then experimental; by `order`, then key.
fn sort_settings(indexes: &mut [usize], settings: &[Setting]) {
    let status = |setting: &Setting| {
        if has_tag(setting, "experimental") {
            2
        } else if has_tag(setting, "preview") {
            1
        } else {
            0
        }
    };
    indexes.sort_by(|&a, &b| {
        let (a, b) = (&settings[a], &settings[b]);
        status(a).cmp(&status(b)).then_with(|| compare_nullable_numbers(order(a), order(b))).then_with(|| locale_compare(&a.key, &b.key))
    });
}

fn order(setting: &Setting) -> Option<f64> {
    setting.schema["order"].as_f64()
}

pub fn tags(setting: &Setting) -> impl Iterator<Item = &str> {
    setting.schema["tags"].as_array().into_iter().flatten().filter_map(Value::as_str)
}

pub fn has_tag(setting: &Setting, tag: &str) -> bool {
    tags(setting).any(|candidate| candidate == tag)
}

/// `SettingValueType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Enum,
    String,
    MultilineString,
    Integer,
    Number,
    Boolean,
    Array,
    Exclude,
    Include,
    Complex,
    NullableInteger,
    NullableNumber,
    Object,
    BooleanObject,
    LanguageTag,
    ComplexObject,
}

/// `initSettingValueType`.
pub fn value_type(setting: &Setting) -> ValueType {
    let schema = &setting.schema;
    let kind = &schema["type"];
    let types: Vec<&str> = match kind {
        Value::String(kind) => vec![kind.as_str()],
        Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let enum_renderable = types.iter().all(|kind| ["string", "boolean", "null", "integer", "number"].contains(kind));
    if setting.enum_values.is_some() && schema["enum"].is_array() && (kind.is_null() || enum_renderable) {
        return ValueType::Enum;
    }
    if kind == "string" {
        return if schema["editPresentation"] == "multilineText" { ValueType::MultilineString } else { ValueType::String };
    }
    if is_exclude_setting(&setting.key) {
        return ValueType::Exclude;
    }
    if setting.key == "files.readonlyInclude" {
        return ValueType::Include;
    }
    match kind.as_str() {
        Some("integer") => return ValueType::Integer,
        Some("number") => return ValueType::Number,
        Some("boolean") => return ValueType::Boolean,
        Some("array") if array_item_type(setting).is_some_and(|item| ["string", "enum", "number", "integer"].contains(&item)) => return ValueType::Array,
        _ => {}
    }
    if kind.is_array() && types.contains(&"null") && types.len() == 2 {
        return if types.contains(&"integer") {
            ValueType::NullableInteger
        } else if types.contains(&"number") {
            ValueType::NullableNumber
        } else {
            ValueType::Complex
        };
    }
    match object_setting_schema_type(setting) {
        Some(_) if all_keys_are_boolean(schema) => ValueType::BooleanObject,
        Some(true) => ValueType::Object,
        Some(false) => ValueType::ComplexObject,
        None if config::model::is_override_key(&setting.key) => ValueType::LanguageTag,
        None => ValueType::Complex,
    }
}

/// `isExcludeSetting`.
fn is_exclude_setting(key: &str) -> bool {
    ["files.exclude", "search.exclude", "workbench.localHistory.exclude", "explorer.autoRevealExclude", "files.readonlyExclude", "files.watcherExclude"].contains(&key)
}

/// `ISetting.arrayItemType` of `parseSetting`.
pub fn array_item_type(setting: &Setting) -> Option<&str> {
    let items = &setting.schema["items"];
    if setting.schema["type"] != "array" || !items.is_object() || items["type"].is_null() {
        return None;
    }
    if items["enum"].is_array() {
        return Some("enum");
    }
    items["type"].as_str()
}

/// `allKeysAreBoolean`.
fn all_keys_are_boolean(schema: &Value) -> bool {
    let additional = &schema["additionalProperties"];
    let properties = schema["properties"].as_object();
    (additional.is_null() || additional == false) && properties.is_some_and(|properties| !properties.is_empty() && properties.values().all(|property| property["type"] == "boolean"))
}

/// `getObjectSettingSchemaType`: `Some(true)` for 'simple', `Some(false)` for 'complex', `None` when not renderable.
fn object_setting_schema_type(setting: &Setting) -> Option<bool> {
    let schema = &setting.schema;
    if schema["type"] != "object" {
        return None;
    }
    let (properties, patterns, additional) = (&schema["properties"], &schema["patternProperties"], &schema["additionalProperties"]);
    if properties.is_null() && patterns.is_null() && additional.is_null() {
        return None;
    }
    let catch_all = patterns.as_object().is_some_and(|patterns| patterns.contains_key(".*"));
    if (additional == true || additional.is_null()) && !catch_all {
        return None;
    }
    let mut schemas: Vec<&Value> = properties.as_object().into_iter().flat_map(|map| map.values()).chain(patterns.as_object().into_iter().flat_map(|map| map.values())).collect();
    if additional.is_object() {
        schemas.push(additional);
    }
    let mut simple = true;
    for schema in schemas {
        let sub_schemas: Vec<&Value> = match schema["anyOf"].as_array() {
            Some(any_of) => any_of.iter().collect(),
            None => vec![schema],
        };
        for sub_schema in sub_schemas {
            match object_renderable_schema_type(sub_schema, &setting.key)? {
                true => {}
                false => simple = false,
            }
        }
    }
    Some(simple)
}

fn is_simple_type(kind: &Value) -> bool {
    ["string", "boolean", "integer", "number"].iter().any(|simple| kind == simple)
}

/// `getObjectRenderableSchemaType`.
fn object_renderable_schema_type(schema: &Value, key: &str) -> Option<bool> {
    let kind = &schema["type"];
    if let Some(kinds) = kind.as_array() {
        let has = |name: &str| kinds.iter().any(|kind| kind == name);
        if key == "workbench.editor.customLabels.patterns" && kinds.len() == 2 && has("null") && (has("string") || has("boolean") || has("integer") || has("number")) {
            return Some(true);
        }
        return kinds.iter().all(is_simple_type).then_some(false);
    }
    if is_simple_type(kind) {
        return Some(true);
    }
    if kind == "array" {
        let items = &schema["items"];
        if items.is_null() {
            return None;
        }
        let item_schemas: Vec<&Value> = match items.as_array() {
            Some(items) => items.iter().collect(),
            None => vec![items],
        };
        for item in item_schemas {
            let item_kind = &item["type"];
            let simple = match item_kind.as_array() {
                Some(kinds) => kinds.iter().all(is_simple_type),
                None => is_simple_type(item_kind),
            };
            if !simple {
                return None;
            }
        }
        return Some(false);
    }
    None
}

/// `knownAcronyms`.
const KNOWN_ACRONYMS: &[&str] = &["css", "html", "scss", "less", "json", "js", "ts", "ie", "id", "php", "scm"];
/// `knownTermMappings`, in insertion order.
const KNOWN_TERM_MAPPINGS: &[(&str, &str)] = &[
    ("power shell", "PowerShell"),
    ("powershell", "PowerShell"),
    ("javascript", "JavaScript"),
    ("typescript", "TypeScript"),
    ("github", "GitHub"),
    ("ocaml", "OCaml"),
    ("jet brains", "JetBrains"),
    ("jetbrains", "JetBrains"),
    ("re sharper", "ReSharper"),
    ("resharper", "ReSharper"),
];

/// `wordifyKey`.
pub fn wordify_key(key: &str) -> String {
    let dots = regex::Regex::new(r"\.([a-z0-9])").expect("valid");
    let key = dots.replace_all(key, |captures: &regex::Captures| format!(" \u{203A} {}", captures[1].to_uppercase()));
    let camel = regex::Regex::new(r"([a-z0-9])([A-Z])").expect("valid");
    let key = camel.replace_all(&key, "$1 $2");
    let capitals = regex::Regex::new(r"([A-Z]+)([A-Z][a-z])").expect("valid");
    let key = capitals.replace_all(&key, "$1 $2");
    let mut key = key.into_owned();
    if let Some(first) = key.chars().next().filter(char::is_ascii_lowercase) {
        key.replace_range(..1, &first.to_ascii_uppercase().to_string());
    }
    let words = regex::Regex::new(r"\b\w+\b").expect("valid");
    let mut key = words.replace_all(&key, |captures: &regex::Captures| if KNOWN_ACRONYMS.contains(&captures[0].to_lowercase().as_str()) { captures[0].to_uppercase() } else { captures[0].to_string() }).into_owned();
    for (term, replacement) in KNOWN_TERM_MAPPINGS {
        let pattern = regex::RegexBuilder::new(&format!(r"\b{term}\b")).case_insensitive(true).build().expect("valid");
        key = pattern.replace_all(&key, *replacement).into_owned();
    }
    key
}

/// `settingKeyToDisplayFormat`: the category before the last dot, trimmed of what the group id says, and the label.
pub fn setting_key_to_display_format(key: &str, group_id: &str, is_language_tag_setting: bool) -> (String, String) {
    let display_label = match key {
        "chat.agent.sandbox.network.allowNetwork" => Some("Allow Outbound Connections"),
        "chat.agent.sandbox.allowUnsandboxedCommands" => Some("Allow Sandbox Bypass"),
        "chat.agent.sandbox.mcpServers" => Some("Sandbox MCP Servers"),
        "chat.agent.sandbox.lspServers" => Some("Sandbox LSP Servers"),
        "chat.agent.sandbox.credentials.authenticategit" => Some("Authenticate git"),
        "chat.agent.sandbox.credentials.authenticategh" => Some("Authenticate gh"),
        "chat.agent.sandbox.credentials.envVars" => Some("Masked environment variables"),
        "chat.agent.sandbox.fileSystem.userConfiguredPaths" => Some("User-Configured Paths"),
        _ => None,
    };
    let (category, last) = match key.rfind('.') {
        Some(index) => (&key[..index], &key[index + 1..]),
        None => ("", key),
    };
    let group_id = group_id.replace('/', ".");
    let category = wordify_key(&trim_category_for_group(category, &group_id));
    if let Some(label) = display_label {
        return (category, label.to_string());
    }
    let mut label_key = last.to_string();
    if is_language_tag_setting {
        let plain = label_key.strip_prefix('[').unwrap_or(&label_key);
        let plain = plain.strip_suffix(']').unwrap_or(plain).replace("][", ", ");
        label_key = format!("$(bracket) {plain}");
    }
    (category, wordify_key(&label_key))
}

/// `trimCategoryForGroup`: drops the leading part of the category the group id already names.
fn trim_category_for_group(category: &str, group_id: &str) -> String {
    let insiders = regex::RegexBuilder::new("-?insiders$").case_insensitive(true).build().expect("valid");
    let group_id = if category.to_lowercase().ends_with("insiders") { group_id.to_string() } else { insiders.replace(group_id, "").into_owned() };
    let parts: Vec<String> = group_id
        .split('.')
        .map(|part| {
            let without_hyphens = part.replace('-', "");
            if without_hyphens.to_lowercase() == category.to_lowercase() { without_hyphens } else { part.to_string() }
        })
        .collect();
    for forward in [true, false] {
        let mut parts = parts.clone();
        while !parts.is_empty() {
            let pattern = regex::RegexBuilder::new(&format!(r"^{}(\.|$)", parts.join(r"\."))).case_insensitive(true).build();
            if let Ok(pattern) = pattern
                && pattern.is_match(category)
            {
                return pattern.replace(category, "").into_owned();
            }
            if forward {
                parts.pop();
            } else {
                parts.remove(0);
            }
        }
    }
    category.to_string()
}

/// `IParsedQuery`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedQuery {
    pub tags: Vec<String>,
    pub query: String,
    pub extension_filters: Vec<String>,
    pub id_filters: Vec<String>,
    pub feature_filters: Vec<String>,
    pub language_filter: Option<String>,
}

impl ParsedQuery {
    pub fn has_filters(&self) -> bool {
        !self.tags.is_empty() || !self.extension_filters.is_empty() || !self.feature_filters.is_empty() || !self.id_filters.is_empty() || self.language_filter.is_some()
    }
}

/// `parseQuery`.
pub fn parse_query(query: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    let tag = regex::Regex::new(r#"(^|\s)@tag:("([^"]*)"|[^"]\S*)"#).expect("valid");
    let mut tags = Vec::new();
    let mut query = tag
        .replace_all(query, |captures: &regex::Captures| {
            tags.push(captures.get(3).map(|quoted| quoted.as_str()).filter(|quoted| !quoted.is_empty()).unwrap_or(&captures[2]).to_string());
            ""
        })
        .into_owned();
    for name in [MODIFIED_SETTING_TAG, POLICY_SETTING_TAG, "override:agentsWindow"] {
        let token = format!("@{name}");
        if let Some(index) = query.find(&token) {
            tags.push(name.to_string());
            query.replace_range(index..index + token.len(), "");
        }
    }
    while let Some(index) = query.find("@stable") {
        tags.push("stable".into());
        query.replace_range(index..index + "@stable".len(), "");
    }
    parsed.tags = tags;
    for (name, parts) in [("ext", &mut parsed.extension_filters), ("feature", &mut parsed.feature_filters), ("id", &mut parsed.id_filters)] {
        query = take_filter(&query, name, parts);
    }
    let mut languages = Vec::new();
    query = take_filter(&query, "lang", &mut languages);
    parsed.language_filter = languages.into_iter().next();
    parsed.query = query.trim().to_string();
    parsed
}

/// `getTagsForType`: removes `@name:a,b` from the query and appends its parts.
fn take_filter(query: &str, name: &str, parts: &mut Vec<String>) -> String {
    let pattern = regex::Regex::new(&format!(r#"(^|\s)@{name}:("([^"]*)"|[^"]\S*)?"#)).expect("valid");
    pattern
        .replace_all(query, |captures: &regex::Captures| {
            let quoted = captures.get(3).map(|quoted| quoted.as_str()).filter(|quoted| !quoted.is_empty());
            if let Some(element) = quoted.or(captures.get(2).map(|whole| whole.as_str())) {
                parts.extend(element.split(',').map(str::trim).filter(|part| !part.is_empty()).map(str::to_string));
            }
            ""
        })
        .into_owned()
}

/// `SettingMatchType`.
pub mod match_type {
    pub const NONE: u32 = 0;
    pub const LANGUAGE_TAG_SETTING_MATCH: u32 = 1 << 0;
    pub const NON_CONTIGUOUS_QUERY_IN_SETTING_ID: u32 = 1 << 2;
    pub const DESCRIPTION_OR_VALUE_MATCH: u32 = 1 << 3;
    pub const NON_CONTIGUOUS_WORDS_IN_SETTINGS_LABEL: u32 = 1 << 4;
    pub const CONTIGUOUS_WORDS_IN_SETTINGS_LABEL: u32 = 1 << 5;
    pub const CONTIGUOUS_QUERY_IN_SETTING_ID: u32 = 1 << 6;
    pub const ALL_WORDS_IN_SETTINGS_LABEL: u32 = 1 << 7;
    pub const EXACT_MATCH: u32 = 1 << 8;
    pub const KEY_MATCH_TYPES: u32 = ALL_WORDS_IN_SETTINGS_LABEL | CONTIGUOUS_WORDS_IN_SETTINGS_LABEL | NON_CONTIGUOUS_WORDS_IN_SETTINGS_LABEL | NON_CONTIGUOUS_QUERY_IN_SETTING_ID | CONTIGUOUS_QUERY_IN_SETTING_ID;
}

/// One `ISettingMatch`: the setting, its `SettingMatchType` bits and the number of query words its key matched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingMatch {
    pub index: usize,
    pub match_type: u32,
    pub key_match_score: usize,
}

/// `SettingMatches._doFindMatchesInSetting`, reduced to the match type and key score. `value` is the setting's
/// current value, searched when it is a string.
pub fn find_matches(search: &str, setting: &Setting, value: Option<&Value>) -> (u32, usize) {
    use match_type::*;
    let mut matched = NONE;
    let mut key_score = 0;
    let key_words = key_to_label(&setting.key);
    let mut query_words: Vec<&str> = Vec::new();
    for word in search.split(' ') {
        if !query_words.contains(&word) {
            query_words.push(word);
        }
    }
    let key_matching = query_words.iter().filter(|word| matches_contiguous_words(word, &key_words).is_some_and(|matches| !matches.is_empty())).count();
    if key_matching == query_words.len() {
        matched |= ALL_WORDS_IN_SETTINGS_LABEL;
    } else if key_matching >= 2 {
        matched |= CONTIGUOUS_WORDS_IN_SETTINGS_LABEL;
        key_score = key_matching;
    }
    let search_alphanumeric = to_alphanumeric(search);
    let key_alphanumeric = to_alphanumeric(&setting.key);
    if matches_contiguous_sub_string(&search_alphanumeric, &key_alphanumeric).is_some_and(|matches| !matches.is_empty()) {
        matched |= CONTIGUOUS_QUERY_IN_SETTING_ID;
    }
    if matched == NONE {
        let key_matching = query_words.iter().filter(|word| matches_words(word, &key_words).is_some_and(|matches| !matches.is_empty())).count();
        if key_matching >= 2 || (key_matching == 1 && query_words.len() == 1) {
            matched |= NON_CONTIGUOUS_WORDS_IN_SETTINGS_LABEL;
            key_score = key_matching;
        } else if matches_sub_string(&search_alphanumeric, &key_alphanumeric).is_some_and(|matches| !matches.is_empty()) {
            matched |= NON_CONTIGUOUS_QUERY_IN_SETTING_ID;
        }
    }
    if !setting.overrides.is_empty() && matched != NONE {
        // A `[language]` setting only counts by its key, and sorts last.
        return (LANGUAGE_TAG_SETTING_MATCH, key_score);
    }
    // Every match type set so far comes with key ranges, so upstream's `matches.length === 0` never drops one.
    if matched < CONTIGUOUS_WORDS_IN_SETTINGS_LABEL {
        let keywords = setting.schema["keywords"].as_array().map(|keywords| keywords.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "));
        let mut lines: Vec<&str> = setting.description.iter().map(String::as_str).collect();
        if let Some(keywords) = keywords.as_deref().filter(|_| setting.schema["keywords"].as_array().is_some_and(|keywords| !keywords.is_empty())) {
            lines.push(keywords);
        }
        let all_in_description = query_words.iter().all(|word| lines.iter().any(|line| matches_base_contiguous_sub_string(word, line).is_some_and(|matches| !matches.is_empty())));
        if all_in_description {
            matched |= DESCRIPTION_OR_VALUE_MATCH;
        }
        let enum_values: Vec<&str> = setting.schema["enum"].as_array().map(|values| values.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let all_in = |text: &str| query_words.iter().all(|word| matches_contiguous_sub_string(word, text).is_some_and(|matches| !matches.is_empty()));
        if setting.enum_values.as_ref().is_some_and(|values| !values.is_empty()) {
            if enum_values.iter().any(|option| all_in(option)) {
                matched |= DESCRIPTION_OR_VALUE_MATCH;
            }
        } else if let Some(Value::String(value)) = value
            && all_in(value)
        {
            matched |= DESCRIPTION_OR_VALUE_MATCH;
        }
    }
    (matched, key_score)
}

/// `_keyToLabel`.
fn key_to_label(key: &str) -> String {
    let separators = regex::Regex::new(r"[-._]").expect("valid");
    let label = separators.replace_all(key, " ");
    let camel = regex::Regex::new(r"([a-z]+)([A-Z])").expect("valid");
    let label = camel.replace_all(&label, "$1 $2");
    let letters_digits = regex::Regex::new(r"([A-Za-z]+)(\d+)").expect("valid");
    let label = letters_digits.replace_all(&label, "$1 $2");
    let digits_letters = regex::Regex::new(r"(\d+)([A-Za-z]+)").expect("valid");
    digits_letters.replace_all(&label, "$1 $2").to_lowercase()
}

/// `_toAlphaNumeric`.
fn to_alphanumeric(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// `cleanFilter`.
fn clean_filter(filter: &str) -> String {
    filter.replace(['"', ':'], " ").replace("  ", " ").trim().to_string()
}

/// `LocalSearchProvider.searchModel`: the matched settings, `None` for an empty filter. `value_of` reads a
/// setting's current value. Groups whose title matches the filter bring all their settings.
pub fn local_search(filter: &str, model: &Model, groups: &[SettingsGroup], value_of: impl Fn(&str) -> Option<Value>) -> Option<(Vec<SettingMatch>, bool)> {
    use match_type::*;
    let filter = clean_filter(filter);
    if filter.is_empty() {
        return None;
    }
    let title_pattern = regex::RegexBuilder::new(&regex::escape(&filter)).case_insensitive(true).build().expect("escaped");
    let mut matches = Vec::new();
    let mut index = 0;
    for group in groups {
        let group_matched = title_pattern.is_match(&group.title);
        for setting in &group.settings {
            let (mut match_type, key_match_score) = find_matches(&filter, setting, value_of(&setting.key).as_ref());
            if match_type != NONE && filter.to_lowercase() == setting.key.to_lowercase() {
                match_type = EXACT_MATCH;
            }
            if group_matched || match_type != NONE {
                matches.push(SettingMatch { index, match_type, key_match_score });
            }
            index += 1;
        }
    }
    debug_assert_eq!(index, model.settings.len());
    let top_key_match = matches.iter().map(|found| found.match_type & KEY_MATCH_TYPES).max().unwrap_or(0);
    let always_allowed = DESCRIPTION_OR_VALUE_MATCH | LANGUAGE_TAG_SETTING_MATCH;
    matches.retain(|found| found.match_type & top_key_match != 0 || found.match_type & always_allowed != 0 || found.match_type == EXACT_MATCH);
    let exact = matches.iter().any(|found| found.match_type == EXACT_MATCH);
    Some((matches, exact))
}

/// `SearchResultModel.sortResults`: by TOC order with only filters, else by match type, key score and TOC order;
/// duplicates dropped.
pub fn sort_results(matches: &mut Vec<SettingMatch>, toc_order: &[Option<usize>], has_query: bool) {
    use match_type::*;
    let toc = |found: &SettingMatch| toc_order[found.index].map(|order| order as f64);
    if has_query {
        matches.sort_by(|a, b| {
            if a.match_type != b.match_type {
                return b.match_type.cmp(&a.match_type);
            }
            if a.match_type & (NON_CONTIGUOUS_WORDS_IN_SETTINGS_LABEL | CONTIGUOUS_WORDS_IN_SETTINGS_LABEL) != 0 {
                return b.key_match_score.cmp(&a.key_match_score).then_with(|| compare_nullable_numbers(toc(a), toc(b)));
            }
            compare_nullable_numbers(toc(a), toc(b))
        });
    } else {
        matches.sort_by(|a, b| compare_nullable_numbers(toc(a), toc(b)));
    }
    let mut seen = HashSet::new();
    matches.retain(|found| seen.insert(found.index));
}

/// `settingsOrderByTocIndex`: each setting's position in a depth-first walk of the tree.
pub fn toc_order(model: &Model) -> Vec<Option<usize>> {
    fn walk(group: &Group, order: &mut Vec<Option<usize>>, next: &mut usize) {
        for &index in &group.settings {
            if order[index].is_none() {
                order[index] = Some(*next);
                *next += 1;
            }
        }
        for child in &group.children {
            walk(child, order, next);
        }
    }
    let mut order = vec![None; model.settings.len()];
    let mut next = 0;
    // `createSettingsOrderByTocIndex` skips Commonly Used.
    for child in model.root.children.iter().skip(1) {
        walk(child, &mut order, &mut next);
    }
    order
}

/// A setting's state in the selected scope: what `SettingsTreeSettingElement.update` derives from `inspect`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inspection {
    pub value: Value,
    pub default_value: Value,
    pub is_configured: bool,
    /// `overriddenScopeList`: `user:` or `workspace:`, then `<scope>:<language>`, where another scope sets it.
    pub overridden_scopes: Vec<String>,
    /// `overriddenDefaultsLanguageList`: the languages whose default overrides set it.
    pub overridden_default_languages: Vec<String>,
    /// `hasPolicyValue`: a policy sets the value, so the control is read-only and shows the policy's value.
    pub has_policy_value: bool,
}

/// `inspectSetting` then `update`: the value shown in `target`'s tab, whether that scope sets it, and the scopes
/// and language blocks that also set it. `folder` is the folder of the Folder tab.
// shortcut: the `@lang:` filter's language selector is not applied to the value; the filter only narrows the list.
pub fn inspect_setting(configuration: &Configuration, key: &str, target: Target, folder: Option<&Path>) -> Inspection {
    let inspected = configuration.inspect(key, &Overrides { resource: folder, override_identifier: None });
    let selected = match target {
        Target::Workspace => &inspected.workspace,
        Target::WorkspaceFolder => &inspected.workspace_folder,
        _ => &inspected.user_local,
    };
    let default_value = inspected.default_value.clone().unwrap_or(Value::Null);
    let has_policy_value = inspected.policy_value.is_some();
    // The user did not set a policy value themselves, so it does not count as configured.
    let (value, is_configured) = match &inspected.policy_value {
        Some(policy_value) => (policy_value.clone(), false),
        None => (selected.value.clone().unwrap_or_else(|| default_value.clone()), selected.value.is_some()),
    };
    let mut overridden_scopes = Vec::new();
    if target != Target::Workspace && inspected.workspace.value.is_some() {
        overridden_scopes.push("workspace:".to_string());
    }
    if !matches!(target, Target::UserLocal | Target::User | Target::Application) && inspected.user_local.value.is_some() {
        overridden_scopes.push("user:".to_string());
    }
    let language_blocks = |model: &ConfigurationModel| -> Vec<String> {
        let blocks = model.overrides.iter().filter(|block| block.keys.iter().any(|block_key| block_key == key));
        blocks.flat_map(|block| block.identifiers.iter().cloned()).collect()
    };
    let overridden_default_languages = language_blocks(&configuration.defaults);
    let workspace = if configuration.has_workspace { language_blocks(&configuration.workspace) } else { Vec::new() };
    let user = language_blocks(&configuration.local_user);
    overridden_scopes.extend(workspace.iter().map(|language| format!("workspace:{language}")));
    overridden_scopes.extend(user.iter().map(|language| format!("user:{language}")));
    Inspection { value, default_value, is_configured, overridden_scopes, overridden_default_languages, has_policy_value }
}

/// One run of `renderSettingMarkdown`'s inline markdown: a code span or a link with its href.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkdownRun {
    Code,
    Link(String),
}

/// `fixSettingLinks`: `` `#key#` `` and `'#key#'` become a link named "Category: Label", or that name quoted.
pub fn fix_setting_links(text: &str, linkify: bool) -> String {
    let pattern = regex::Regex::new(r"`#([^#\s`]+)#`|'#([^#\s']+)#'").expect("the setting link pattern compiles");
    pattern
        .replace_all(text, |captures: &regex::Captures| {
            let key = captures.get(1).or_else(|| captures.get(2)).map_or("", |key| key.as_str());
            let (category, label) = setting_key_to_display_format(key, "", false);
            match linkify {
                true => format!("[{category}: {label}](#{key} \"{key}\")"),
                false => format!("\"{category}: {label}\""),
            }
        })
        .into_owned()
}

/// The plain text of a setting description and its code and link runs, as byte ranges of that text.
// shortcut: inline code and links only; emphasis, lists and fenced blocks show their source text. Upgrade with a
// markdown renderer shared with hovers.
pub fn markdown_runs(markdown: &str) -> (String, Vec<(std::ops::Range<usize>, MarkdownRun)>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut rest = markdown;
    while let Some(start) = rest.find(['`', '[']) {
        text.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if rest[start..].starts_with('`')
            && let Some(end) = after.find('`')
        {
            let code = &after[..end];
            runs.push((text.len()..text.len() + code.len(), MarkdownRun::Code));
            text.push_str(code);
            rest = &after[end + 1..];
            continue;
        }
        if rest[start..].starts_with('[')
            && let Some(close) = after.find("](")
            && let Some(end) = after[close + 2..].find(')')
        {
            let label = &after[..close];
            let target = &after[close + 2..close + 2 + end];
            let href = target.split_once(" \"").map_or(target, |(href, _)| href);
            runs.push((text.len()..text.len() + label.len(), MarkdownRun::Link(href.to_string())));
            text.push_str(label);
            rest = &after[close + 2 + end + 1..];
            continue;
        }
        text.push_str(&rest[start..start + 1]);
        rest = after;
    }
    text.push_str(rest);
    (text, runs)
}

/// `matchesAllTags` with the setting's `tags` set: its schema tags, `modified` when configured and `hasPolicy`
/// under a policy.
pub fn matches_all_tags(setting: &Setting, inspection: &Inspection, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let is_configured = inspection.is_configured;
    let has = |tag: &str| {
        (tag == MODIFIED_SETTING_TAG && is_configured)
            || (tag == POLICY_SETTING_TAG && inspection.has_policy_value)
            || (tag == REQUIRE_TRUSTED_WORKSPACE_SETTING_TAG && setting.restricted)
            || has_tag(setting, tag)
    };
    let any_tags = is_configured || inspection.has_policy_value || setting.restricted || tags(setting).next().is_some();
    if filters.iter().any(|tag| tag == "stable") {
        if has_tag(setting, "preview") || has_tag(setting, "experimental") {
            return false;
        }
        let others: Vec<&String> = filters.iter().filter(|tag| *tag != "stable").collect();
        return others.is_empty() || (any_tags && others.iter().all(|tag| has(tag)));
    }
    any_tags && filters.iter().all(|tag| has(tag))
}

/// `matchesScope`: whether the setting can be written in the target.
pub fn matches_scope(setting: &Setting, target: Target) -> bool {
    let Some(scope) = setting.scope else { return true };
    match target {
        Target::Application => APPLICATION_SCOPES.contains(&scope),
        Target::WorkspaceFolder => FOLDER_SCOPES.contains(&scope),
        Target::Workspace => WORKSPACE_SCOPES.contains(&scope),
        _ => true,
    }
}

/// `matchesAnyExtension`.
pub fn matches_any_extension(setting: &Setting, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let Some(extension_id) = &setting.extension_id else { return false };
    filters.iter().any(|filter| filter.to_lowercase() == extension_id.to_lowercase())
}

/// `matchesAnyFeature`: a core setting under `features/<filter>`, or under any Chat entry for `chat`.
pub fn matches_any_feature(setting: &Setting, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    if setting.extension_id.is_some() {
        return false;
    }
    let matches_node = |node: &TocNode| node.settings.iter().any(|pattern| setting_match_regex(pattern).is_match(&setting.key));
    if filters.iter().any(|filter| filter == "chat")
        && let Some(chat) = TOC_DATA.iter().find(|node| node.id == "chat")
        && chat.children.iter().any(matches_node)
    {
        return true;
    }
    let features = TOC_DATA.iter().find(|node| node.id == "features").expect("tocData has features");
    filters.iter().any(|filter| features.children.iter().find(|feature| feature.id == format!("features/{filter}")).is_some_and(matches_node))
}

/// `matchesAnyId`: the key, or a prefix ending in `*`.
pub fn matches_any_id(setting: &Setting, filters: &[String]) -> bool {
    if filters.is_empty() || filters.contains(&setting.key) {
        return true;
    }
    filters.iter().any(|filter| filter.strip_suffix('*').is_some_and(|prefix| setting.key.starts_with(prefix)))
}

/// `matchesAllLanguages`: a known language shows the language-overridable settings.
pub fn matches_all_languages(setting: &Setting, language_filter: Option<&str>, is_language: impl Fn(&str) -> bool) -> bool {
    let Some(language) = language_filter else { return true };
    is_language(language) && setting.scope == Some(Scope::LanguageOverridable)
}

/// `shouldShowSetting`: an advanced setting shows when an id filter, the query or `@hasPolicy` names it.
pub fn should_show_setting(setting: &Setting, parsed: &ParsedQuery, raw_query: &str) -> bool {
    !has_tag(setting, ADVANCED_SETTING_TAG)
        || parsed.id_filters.contains(&setting.key)
        || raw_query.to_lowercase().contains(&setting.key.to_lowercase())
        || parsed.tags.iter().any(|tag| tag == POLICY_SETTING_TAG)
}

/// `canShowAdvancedSettings`.
pub fn can_show_advanced(always_show: bool, parsed: &ParsedQuery) -> bool {
    always_show || parsed.tags.iter().any(|tag| tag == ADVANCED_SETTING_TAG || tag == "expassigned")
}

/// One row of the settings tree: a group title at its depth (1 for the top level), or a setting with the id of the
/// group it shows under (`searchResultModel` in search results), which trims its category.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Group { id: String, label: String, level: usize },
    Setting(usize, String),
}

/// One row of the table of contents: a group with its count of shown settings.
#[derive(Clone, Debug, PartialEq)]
pub struct TocRow {
    pub id: String,
    pub label: String,
    pub level: usize,
    pub count: usize,
    pub has_children: bool,
}

/// The parts of `ISettingsEditorViewState` the filters read.
pub struct Filters<'a> {
    pub parsed: &'a ParsedQuery,
    pub raw_query: &'a str,
    pub target: Target,
    /// Each setting's state in the target, by setting index.
    pub inspections: &'a [Inspection],
    pub always_show_advanced: bool,
}

/// `triggerSearch`: the settings found, in result order; `None` without a query or a filter, where the tree browses.
/// `value_of` reads a setting's current value, `is_language` tells a known language id.
pub fn search(model: &Model, groups: &[SettingsGroup], filters: &Filters, value_of: impl Fn(&str) -> Option<Value>, is_language: impl Fn(&str) -> bool) -> Option<Vec<usize>> {
    let parsed = filters.parsed;
    let has_query = !parsed.query.is_empty() && parsed.query != "@";
    if !has_query && !parsed.has_filters() {
        return None;
    }
    let show_advanced = can_show_advanced(filters.always_show_advanced, parsed);
    // `createFilterModel` keeps every setting; `triggerFilterPreferences` keeps the local matches.
    let mut matches = match has_query {
        true => local_search(&parsed.query, model, groups, value_of).map_or_else(Vec::new, |(matches, _)| matches),
        false => (0..model.settings.len()).map(|index| SettingMatch { index, match_type: match_type::NONE, key_match_score: 0 }).collect(),
    };
    matches.retain(|found| show_advanced || should_show_setting(&model.settings[found.index], parsed, filters.raw_query));
    sort_results(&mut matches, &toc_order(model), has_query);
    // `SearchResultModel.updateChildren`.
    let shown = matches.into_iter().map(|found| found.index).filter(|&index| {
        let setting = &model.settings[index];
        matches_all_tags(setting, &filters.inspections[index], &parsed.tags)
            && matches_scope(setting, filters.target)
            && matches_any_extension(setting, &parsed.extension_filters)
            && matches_any_id(setting, &parsed.id_filters)
            && matches_any_feature(setting, &parsed.feature_filters)
            && matches_all_languages(setting, parsed.language_filter.as_deref(), &is_language)
    });
    Some(shown.collect())
}

pub fn find_group<'a>(group: &'a Group, id: &str) -> Option<&'a Group> {
    if group.id == id {
        return Some(group);
    }
    group.children.iter().find_map(|child| find_group(child, id))
}

/// `expandAll` of the TOC: every group id below `group`.
pub fn collect_group_ids(group: &Group, ids: &mut HashSet<String>) {
    for child in &group.children {
        ids.insert(child.id.clone());
        collect_group_ids(child, ids);
    }
}

fn contains_setting(group: &Group, index: usize) -> bool {
    group.settings.contains(&index) || group.children.iter().any(|child| contains_setting(child, index))
}

/// `SettingsTreeFilter` on the tree: with a category, its titles and its ancestors' and only its settings;
/// settings the target cannot write are hidden, and so are groups left without settings.
pub fn tree_rows(model: &Model, search: Option<&[usize]>, category: Option<&str>, target: Target) -> Vec<Row> {
    let category = category.and_then(|id| find_group(&model.root, id));
    let in_category = |index: usize| category.is_none_or(|category| contains_setting(category, index));
    if let Some(found) = search {
        return found.iter().copied().filter(|&index| in_category(index)).map(|index| Row::Setting(index, "searchResultModel".into())).collect();
    }
    let visible = |index: usize| in_category(index) && matches_scope(&model.settings[index], target);
    let mut rows = Vec::new();
    for child in &model.root.children {
        push_group_rows(child, 1, &visible, &mut rows);
    }
    rows
}

fn push_group_rows(group: &Group, level: usize, visible: &impl Fn(usize) -> bool, rows: &mut Vec<Row>) {
    let start = rows.len();
    rows.push(Row::Group { id: group.id.clone(), label: group.label.clone(), level });
    rows.extend(group.settings.iter().copied().filter(|&index| visible(index)).map(|index| Row::Setting(index, group.id.clone())));
    for child in &group.children {
        push_group_rows(child, level + 1, visible, rows);
    }
    if rows.len() == start + 1 {
        rows.truncate(start);
    }
}

/// `TOCTreeModel.updateGroupCount` and the TOC's filter: groups with shown settings, children of `expanded` ones.
pub fn toc_rows(model: &Model, search: Option<&[usize]>, target: Target, expanded: &HashSet<String>) -> Vec<TocRow> {
    let counted = |index: usize| search.is_none_or(|found| found.contains(&index)) && matches_scope(&model.settings[index], target);
    let mut rows = Vec::new();
    for child in &model.root.children {
        push_toc_rows(child, 1, &counted, expanded, &mut rows);
    }
    rows
}

fn group_count(group: &Group, counted: &impl Fn(usize) -> bool) -> usize {
    group.settings.iter().filter(|&&index| counted(index)).count() + group.children.iter().map(|child| group_count(child, counted)).sum::<usize>()
}

fn push_toc_rows(group: &Group, level: usize, counted: &impl Fn(usize) -> bool, expanded: &HashSet<String>, rows: &mut Vec<TocRow>) {
    let count = group_count(group, counted);
    if count == 0 {
        return;
    }
    let has_children = group.children.iter().any(|child| group_count(child, counted) > 0);
    rows.push(TocRow { id: group.id.clone(), label: group.label.clone(), level, count, has_children });
    if has_children && expanded.contains(&group.id) {
        for child in &group.children {
            push_toc_rows(child, level + 1, counted, expanded, rows);
        }
    }
}

/// The TOC entry of a setting for `onDidClickSetting`: the deepest group holding it, Commonly Used last.
pub fn group_of(model: &Model, index: usize) -> Option<&Group> {
    fn deepest(group: &Group, index: usize) -> Option<&Group> {
        group.children.iter().find_map(|child| deepest(child, index)).or_else(|| group.settings.contains(&index).then_some(group))
    }
    model.root.children.iter().skip(1).find_map(|child| deepest(child, index)).or_else(|| model.root.children.first().filter(|common| common.settings.contains(&index)))
}

/// The ancestors of the group `id`, outermost first, for expanding the TOC to it.
pub fn group_path(model: &Model, id: &str) -> Vec<String> {
    fn walk(group: &Group, id: &str, path: &mut Vec<String>) -> bool {
        if group.id == id {
            return true;
        }
        path.push(group.id.clone());
        if group.children.iter().any(|child| walk(child, id, path)) {
            return true;
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    model.root.children.iter().any(|child| walk(child, id, &mut path));
    path
}

/// `SettingsEditor2.SUGGESTIONS`: the search box's `@` completions.
const SEARCH_SUGGESTIONS: &[&str] = &[
    "@modified",
    "@tag:notebookLayout",
    "@tag:notebookOutputLayout",
    "@tag:requireTrustedWorkspace",
    "@tag:workspaceTrust",
    "@tag:sync",
    "@tag:usesOnlineServices",
    "@tag:telemetry",
    "@tag:accessibility",
    "@tag:preview",
    "@tag:experimental",
    "@tag:expassigned",
    "@tag:agentMerge",
    "@tag:chatPermissions",
    "@tag:advanced",
    "@id:",
    "@ext:",
    "@feature:scm",
    "@feature:explorer",
    "@feature:search",
    "@feature:debug",
    "@feature:extensions",
    "@feature:terminal",
    "@feature:task",
    "@feature:problems",
    "@feature:output",
    "@feature:comments",
    "@feature:remote",
    "@feature:timeline",
    "@feature:notebook",
    "@feature:chat",
    "@hasPolicy",
];

/// `provideResults` of the search box: languages after `@lang:`, extensions after `@ext:`, else the tags not yet in the query.
pub fn search_suggestions(query: &str, language_ids: &[&str], extension_ids: &[&str]) -> Vec<String> {
    let last = query.split(char::is_whitespace).last().unwrap_or_default();
    let unused_sorted = |prefix: &str, ids: &[&str]| {
        let mut tags: Vec<String> = ids.iter().map(|id| format!("{prefix}{id} ")).collect();
        tags.sort();
        tags.retain(|tag| !query.contains(tag.as_str()));
        tags
    };
    if last.starts_with("@lang:") {
        return unused_sorted("@lang:", language_ids);
    }
    if last.starts_with("@ext:") {
        return unused_sorted("@ext:", extension_ids);
    }
    if !query.is_empty() && !last.starts_with('@') {
        return Vec::new();
    }
    let unused = SEARCH_SUGGESTIONS.iter().filter(|tag| !query.contains(**tag));
    unused.map(|tag| if tag.ends_with(':') { tag.to_string() } else { format!("{tag} ") }).collect()
}
