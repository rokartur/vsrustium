//! `ConfigurationMigrationWorkbenchContribution` (workbench/common/configuration.ts) with the migrations of the
//! codeEditor contribution: editorSettingsMigration.ts over migrateOptions.ts, and diffEditorHelper.ts.

use config::model::{Change, ConfigurationModel, get_configuration_value, merge_changes};
use config::service::{Settings, Target};
use gpui::Context;
use serde_json::{Value, json};

use crate::workbench::Workbench;

/// `ConfigurationKeyValuePairs`: `None` removes the key.
type Pairs = Vec<(String, Option<Value>)>;

struct Migration {
    key: String,
    migrate: Box<dyn Fn(&Value, &dyn Fn(&str) -> Option<Value>) -> Pairs>,
}

impl Workbench {
    /// Rewrites the legacy values of every migration in user, workspace and folder settings.
    pub fn migrate_configurations(&mut self, cx: &mut Context<Self>) {
        let before = self.model_creation_options();
        let change = migrate_configurations(&mut self.settings.lock().unwrap());
        if !change.is_empty() {
            self.configuration_changed(&change, before, cx);
        }
    }
}

/// `migrateConfigurations`: each migration over each target's value and its `[language]` values, then
/// `updateValue` of the pairs it returns; a failed write is skipped (`Promise.allSettled`).
pub(crate) fn migrate_configurations(settings: &mut Settings) -> Change {
    let mut changes = Vec::new();
    for migration in migrations() {
        for (target, resource) in targets(settings) {
            let Some(model) = target_model(settings, target, resource.as_deref()) else { continue };
            let mut writes: Vec<((String, Option<Value>), Vec<String>)> = Vec::new();
            if let Some(value) = model.get_value(Some(&migration.key)) {
                let read = |key: &str| model.get_value(Some(key));
                writes.extend((migration.migrate)(&value, &read).into_iter().map(|pair| (pair, Vec::new())));
            }
            for block in &model.overrides {
                let Some(value) = get_configuration_value(&block.contents, &migration.key) else { continue };
                let read = |key: &str| get_configuration_value(&block.contents, key).cloned();
                writes.extend((migration.migrate)(value, &read).into_iter().map(|pair| (pair, block.identifiers.clone())));
            }
            for ((key, value), identifiers) in writes {
                if let Ok(change) = settings.update_value(&key, value.as_ref(), target, resource.as_deref(), &identifiers) {
                    changes.push(change);
                }
            }
        }
    }
    merge_changes(&changes)
}

/// The targets upstream inspects: user and workspace, and each folder in a multi-root workspace.
fn targets(settings: &Settings) -> Vec<(Target, Option<std::path::PathBuf>)> {
    let mut targets = vec![(Target::User, None)];
    let Some(workspace) = &settings.workspace else { return targets };
    targets.push((Target::Workspace, None));
    if workspace.file.is_some() {
        targets.extend(workspace.folders.iter().map(|folder| (Target::WorkspaceFolder, Some(folder.path.clone()))));
    }
    targets
}

fn target_model(settings: &Settings, target: Target, resource: Option<&std::path::Path>) -> Option<ConfigurationModel> {
    let configuration = &settings.configuration;
    match target {
        Target::WorkspaceFolder => configuration.folders.iter().find(|(folder, _)| Some(folder.as_path()) == resource).map(|(_, model)| model.clone()),
        Target::Workspace => Some(configuration.workspace.clone()),
        _ => Some(configuration.local_user.clone()),
    }
}

fn migrations() -> Vec<Migration> {
    let mut migrations: Vec<Migration> = editor_migrations()
        .into_iter()
        .map(|(key, migrate)| Migration {
            key: format!("editor.{key}"),
            migrate: Box::new(move |value, read| {
                let mut pairs = Pairs::new();
                migrate(value, &|key| read(&format!("editor.{key}")), &mut |key, value| pairs.push((format!("editor.{key}"), value)));
                pairs
            }),
        })
        .collect();
    // diffEditorHelper.ts
    migrations.push(Migration {
        key: "diffEditor.experimental.collapseUnchangedRegions".into(),
        migrate: Box::new(|value, _| vec![("diffEditor.hideUnchangedRegions.enabled".into(), Some(value.clone())), ("diffEditor.experimental.collapseUnchangedRegions".into(), None)]),
    });
    migrations
}

type EditorMigration = Box<dyn Fn(&Value, &dyn Fn(&str) -> Option<Value>, &mut dyn FnMut(&str, Option<Value>))>;

/// `EditorSettingMigration.items` of migrateOptions.ts, keys without `editor.`.
fn editor_migrations() -> Vec<(&'static str, EditorMigration)> {
    let simple: [(&str, Vec<(Value, Value)>); 17] = [
        ("wordWrap", vec![(json!(true), json!("on")), (json!(false), json!("off"))]),
        ("lineNumbers", vec![(json!(true), json!("on")), (json!(false), json!("off"))]),
        ("cursorBlinking", vec![(json!("visible"), json!("solid"))]),
        ("renderWhitespace", vec![(json!(true), json!("boundary")), (json!(false), json!("none"))]),
        ("renderLineHighlight", vec![(json!(true), json!("line")), (json!(false), json!("none"))]),
        ("acceptSuggestionOnEnter", vec![(json!(true), json!("on")), (json!(false), json!("off"))]),
        ("tabCompletion", vec![(json!(false), json!("off")), (json!(true), json!("onlySnippets"))]),
        ("hover", vec![(json!(true), json!({ "enabled": true })), (json!(false), json!({ "enabled": false }))]),
        ("parameterHints", vec![(json!(true), json!({ "enabled": true })), (json!(false), json!({ "enabled": false }))]),
        ("autoIndent", vec![(json!(false), json!("advanced")), (json!(true), json!("full"))]),
        ("matchBrackets", vec![(json!(true), json!("always")), (json!(false), json!("never"))]),
        ("renderFinalNewline", vec![(json!(true), json!("on")), (json!(false), json!("off"))]),
        ("cursorSmoothCaretAnimation", vec![(json!(true), json!("on")), (json!(false), json!("off"))]),
        ("occurrencesHighlight", vec![(json!(true), json!("singleFile")), (json!(false), json!("off"))]),
        ("wordBasedSuggestions", vec![(json!(true), json!("matchingDocuments")), (json!(false), json!("off"))]),
        ("defaultColorDecorators", vec![(json!(true), json!("auto")), (json!(false), json!("never"))]),
        ("minimap.autohide", vec![(json!(true), json!("mouseover")), (json!(false), json!("none"))]),
    ];
    let mut items: Vec<(&'static str, EditorMigration)> = Vec::new();
    for (key, values) in simple {
        items.push((
            key,
            Box::new(move |value, _, write| {
                if let Some((_, new_value)) = values.iter().find(|(old_value, _)| old_value == value) {
                    write(key, Some(new_value.clone()));
                }
            }),
        ));
    }
    items.push((
        "autoClosingBrackets",
        Box::new(|value, read, write| {
            if *value == json!(false) {
                write("autoClosingBrackets", Some(json!("never")));
                if read("autoClosingQuotes").is_none() {
                    write("autoClosingQuotes", Some(json!("never")));
                }
                if read("autoSurround").is_none() {
                    write("autoSurround", Some(json!("never")));
                }
            }
        }),
    ));
    for (old, new) in [("renderIndentGuides", "guides.indentation"), ("highlightActiveIndentGuide", "guides.highlightActiveIndentation")] {
        items.push((
            old,
            Box::new(move |value, read, write| {
                write(old, None);
                if read(new).is_none() {
                    write(new, Some(json!(is_truthy(value))));
                }
            }),
        ));
    }
    items.push((
        "suggest.filteredTypes",
        Box::new(|value, read, write| {
            let Value::Object(types) = value else { return };
            for (kind, setting) in SUGGEST_FILTERED_TYPES {
                if types.get(*kind) == Some(&json!(false)) && read(&format!("suggest.{setting}")).is_none() {
                    write(&format!("suggest.{setting}"), Some(json!(false)));
                }
            }
            write("suggest.filteredTypes", None);
        }),
    ));
    items.push((
        "quickSuggestions",
        Box::new(|value, _, write| {
            let Value::Bool(enabled) = value else { return };
            let value = if *enabled { "on" } else { "off" };
            write("quickSuggestions", Some(json!({ "comments": value, "strings": value, "other": value })));
        }),
    ));
    // Each moves a boolean (or number) to a new key unless the new key is set; upstream's key for the edit context one
    // carries `editor.` twice, so it reads `editor.editor.experimentalEditContextEnabled`.
    let moves: [(&'static str, &'static str, fn(&Value) -> bool); 4] = [
        ("experimental.stickyScroll.enabled", "stickyScroll.enabled", Value::is_boolean),
        ("experimental.stickyScroll.maxLineCount", "stickyScroll.maxLineCount", Value::is_number),
        ("editor.experimentalEditContextEnabled", "editor.editContext", Value::is_boolean),
        ("codeActionWidget.includeNearbyQuickfixes", "codeActionWidget.includeNearbyQuickFixes", Value::is_boolean),
    ];
    for (old, new, applies) in moves {
        items.push((
            old,
            Box::new(move |value, read, write| {
                if !applies(value) {
                    return;
                }
                write(old, None);
                if read(new).is_none() {
                    write(new, Some(value.clone()));
                }
            }),
        ));
    }
    items.push((
        "codeActionsOnSave",
        Box::new(|value, _, write| {
            let Value::Object(actions) = value else { return };
            if !actions.values().any(Value::is_boolean) {
                return;
            }
            let mut migrated = actions.clone();
            for action in migrated.values_mut() {
                if let Value::Bool(enabled) = action {
                    *action = json!(if *enabled { "explicit" } else { "never" });
                }
            }
            write("codeActionsOnSave", Some(Value::Object(migrated)));
        }),
    ));
    items.push((
        "lightbulb.enabled",
        Box::new(|value, _, write| {
            if let Value::Bool(enabled) = value {
                write("lightbulb.enabled", if *enabled { None } else { Some(json!("off")) });
            }
        }),
    ));
    items.push((
        "inlineSuggest.edits.codeShifting",
        Box::new(|value, _, write| {
            if let Value::Bool(enabled) = value {
                write("inlineSuggest.edits.codeShifting", None);
                write("inlineSuggest.edits.allowCodeShifting", Some(json!(if *enabled { "always" } else { "never" })));
            }
        }),
    ));
    items.push((
        "hover.enabled",
        Box::new(|value, _, write| {
            if let Value::Bool(enabled) = value {
                write("hover.enabled", Some(json!(if *enabled { "on" } else { "off" })));
            }
        }),
    ));
    items
}

const SUGGEST_FILTERED_TYPES: &[(&str, &str)] = &[
    ("method", "showMethods"),
    ("function", "showFunctions"),
    ("constructor", "showConstructors"),
    ("deprecated", "showDeprecated"),
    ("field", "showFields"),
    ("variable", "showVariables"),
    ("class", "showClasses"),
    ("struct", "showStructs"),
    ("interface", "showInterfaces"),
    ("module", "showModules"),
    ("property", "showProperties"),
    ("event", "showEvents"),
    ("operator", "showOperators"),
    ("unit", "showUnits"),
    ("value", "showValues"),
    ("constant", "showConstants"),
    ("enum", "showEnums"),
    ("enumMember", "showEnumMembers"),
    ("keyword", "showKeywords"),
    ("text", "showWords"),
    ("color", "showColors"),
    ("file", "showFiles"),
    ("reference", "showReferences"),
    ("folder", "showFolders"),
    ("typeParameter", "showTypeParameters"),
    ("snippet", "showSnippets"),
];

/// JavaScript's `!!value`.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::configuration::Overrides;
    use config::registry::Registry;
    use config::workspace::Workspace;

    fn settings_with(name: &str, user: &str, folder_settings: &str) -> (std::path::PathBuf, Settings) {
        let dir = std::env::temp_dir().join(format!("code-gpui-configuration-migration-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("User")).unwrap();
        std::fs::create_dir_all(dir.join("folder/.vscode")).unwrap();
        std::fs::write(dir.join("User/settings.json"), user).unwrap();
        std::fs::write(dir.join("folder/.vscode/settings.json"), folder_settings).unwrap();
        let settings = Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(Workspace::open(dir.join("folder"))));
        (dir, settings)
    }

    #[test]
    fn legacy_editor_values_are_rewritten_in_user_and_workspace_settings() {
        let user = r#"{
	"editor.wordWrap": true,
	"editor.renderIndentGuides": false,
	"[markdown]": {
		"editor.lineNumbers": false
	}
}"#;
        let (dir, mut settings) = settings_with("rewrite", user, r#"{ "editor.quickSuggestions": false, "diffEditor.experimental.collapseUnchangedRegions": true }"#);
        let change = migrate_configurations(&mut settings);
        assert!(change.keys.contains(&"editor.wordWrap".to_string()));

        let user = &settings.configuration.local_user;
        assert_eq!(user.get_value(Some("editor.wordWrap")), Some(json!("on")));
        assert_eq!(user.get_value(Some("editor.renderIndentGuides")), None);
        assert_eq!(user.get_value(Some("editor.guides.indentation")), Some(json!(false)));
        assert_eq!(user.get_override_value(Some("editor.lineNumbers"), "markdown"), Some(json!("off")));
        let workspace = &settings.configuration.workspace;
        assert_eq!(workspace.get_value(Some("editor.quickSuggestions")), Some(json!({ "comments": "off", "strings": "off", "other": "off" })));
        assert_eq!(workspace.get_value(Some("diffEditor.hideUnchangedRegions.enabled")), Some(json!(true)));
        assert_eq!(workspace.get_value(Some("diffEditor.experimental.collapseUnchangedRegions")), None);
        assert_eq!(settings.get_value(Some("editor.wordWrap"), &Overrides::default()), Some(json!("on")));
        assert!(std::fs::read_to_string(dir.join("User/settings.json")).unwrap().contains(r#""editor.wordWrap": "on""#));

        assert!(migrate_configurations(&mut settings).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_moved_value_keeps_the_new_key_already_set() {
        let user = r#"{ "editor.autoClosingBrackets": false, "editor.autoSurround": "quotes", "editor.codeActionsOnSave": { "source.fixAll": true, "source.organizeImports": "always" } }"#;
        let (dir, mut settings) = settings_with("keep", user, "{}");
        migrate_configurations(&mut settings);
        let user = &settings.configuration.local_user;
        assert_eq!(user.get_value(Some("editor.autoClosingBrackets")), Some(json!("never")));
        assert_eq!(user.get_value(Some("editor.autoClosingQuotes")), Some(json!("never")));
        assert_eq!(user.get_value(Some("editor.autoSurround")), Some(json!("quotes")));
        assert_eq!(user.get_value(Some("editor.codeActionsOnSave")), Some(json!({ "source.fixAll": "explicit", "source.organizeImports": "always" })));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
