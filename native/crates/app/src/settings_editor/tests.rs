use config::registry::Registry;
use config::service::Target;
use serde_json::json;

use super::*;
use crate::default_settings::registered_groups;

fn display(key: &str, group: &str) -> (String, String) {
    setting_key_to_display_format(key, group, false)
}

fn pair(category: &str, label: &str) -> (String, String) {
    (category.to_string(), label.to_string())
}

#[test]
fn setting_key_to_display_format_wordifies_the_category_and_label() {
    assert_eq!(display("foo.bar", ""), pair("Foo", "Bar"));
    assert_eq!(display("foo.bar.etc", ""), pair("Foo \u{203A} Bar", "Etc"));
    assert_eq!(display("fooBar.etcSomething", ""), pair("Foo Bar", "Etc Something"));
    assert_eq!(display("foo", ""), pair("", "Foo"));
    assert_eq!(display("foo.1leading.number", ""), pair("Foo \u{203A} 1leading", "Number"));
    assert_eq!(display("foo.1Leading.number", ""), pair("Foo \u{203A} 1 Leading", "Number"));
}

#[test]
fn setting_key_to_display_format_trims_what_the_group_names() {
    assert_eq!(display("foo.bar", "foo"), pair("", "Bar"));
    assert_eq!(display("disableligatures.ligatures", "disableligatures"), pair("", "Ligatures"));
    assert_eq!(display("foo.bar.etc", "foo"), pair("Bar", "Etc"));
    assert_eq!(display("fooBar.etcSomething", "foo"), pair("Foo Bar", "Etc Something"));
    assert_eq!(display("foo.bar.etc", "foo/bar"), pair("", "Etc"));
    assert_eq!(display("foo.bar.etc", "something/foo"), pair("Bar", "Etc"));
    assert_eq!(display("bar.etc", "something.bar"), pair("", "Etc"));
    assert_eq!(display("fooBar.etc", "fooBar"), pair("", "Etc"));
    assert_eq!(display("fooBar.somethingElse.etc", "fooBar"), pair("Something Else", "Etc"));
}

#[test]
fn setting_key_to_display_format_knows_acronyms_terms_and_sandbox_labels() {
    assert_eq!(display("css.someCssSetting", ""), pair("CSS", "Some CSS Setting"));
    assert_eq!(display("powershell.somePowerShellSetting", ""), pair("PowerShell", "Some PowerShell Setting"));
    assert_eq!(display("ocaml.server.extendedHover", ""), pair("OCaml \u{203A} Server", "Extended Hover"));
    assert_eq!(display("chat.agent.sandbox.mcpServers", "chat.agent.sandbox"), pair("", "Sandbox MCP Servers"));
    let key = "chat.agent.sandbox.fileSystem.allowDevToolAccess";
    let shown: Vec<_> = ["", "chat", "chat.agent.sandbox", "chat.agent.sandbox.fileSystem"].iter().map(|group| display(key, group)).collect();
    assert_eq!(
        shown,
        [
            pair("Chat \u{203A} Agent \u{203A} Sandbox \u{203A} File System", "Allow Dev Tool Access"),
            pair("Agent \u{203A} Sandbox \u{203A} File System", "Allow Dev Tool Access"),
            pair("File System", "Allow Dev Tool Access"),
            pair("", "Allow Dev Tool Access"),
        ]
    );
    assert_eq!(setting_key_to_display_format("[css][scss]", "", true), pair("", "$(bracket) CSS, SCSS"));
}

fn parsed(tags: &[&str], query: &str) -> ParsedQuery {
    ParsedQuery { tags: tags.iter().map(|tag| tag.to_string()).collect(), query: query.into(), ..ParsedQuery::default() }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn parse_query_takes_out_tags_and_filters() {
    assert_eq!(parse_query(""), parsed(&[], ""));
    assert_eq!(parse_query("@modified"), parsed(&["modified"], ""));
    assert_eq!(parse_query("@tag:foo"), parsed(&["foo"], ""));
    assert_eq!(parse_query("@modified foo"), parsed(&["modified"], "foo"));
    assert_eq!(parse_query("@tag:foo @modified"), parsed(&["foo", "modified"], ""));
    assert_eq!(parse_query("@tag:foo @modified my query"), parsed(&["foo", "modified"], "my query"));
    assert_eq!(parse_query("test @modified query"), parsed(&["modified"], "test  query"));
    assert_eq!(parse_query("test @modified"), parsed(&["modified"], "test"));
    assert_eq!(parse_query("query has @ for some reason"), parsed(&[], "query has @ for some reason"));
    assert_eq!(parse_query("@ext:github.vscode-pull-request-github").extension_filters, strings(&["github.vscode-pull-request-github"]));
    assert_eq!(parse_query("@ext:github.vscode-pull-request-github,vscode.git").extension_filters, strings(&["github.vscode-pull-request-github", "vscode.git"]));
    assert_eq!(parse_query("@feature:scm").feature_filters, strings(&["scm"]));
    assert_eq!(parse_query("@feature:scm,terminal").feature_filters, strings(&["scm", "terminal"]));
    assert_eq!(parse_query("@id:files.autoSave").id_filters, strings(&["files.autoSave"]));
    assert_eq!(parse_query("@id:files.autoSave,terminal.integrated.commandsToSkipShell").id_filters, strings(&["files.autoSave", "terminal.integrated.commandsToSkipShell"]));
    assert_eq!(parse_query("@lang:cpp").language_filter.as_deref(), Some("cpp"));
    assert_eq!(parse_query("@lang:cpp,python").language_filter.as_deref(), Some("cpp"));
    assert_eq!(parse_query("@tag:\"a b\" x"), parsed(&["a b"], "x"));
}

fn sample_groups() -> Vec<SettingsGroup> {
    let mut registry = Registry::default();
    let contribution = json!({ "contributes": { "configuration": { "title": "Sample", "properties": {
        "sample.flag": { "type": "boolean", "default": false, "description": "Turns the flag on." },
        "sample.mode": { "type": "string", "enum": ["one", "two"], "default": "one", "description": "Picks a mode." },
        "sample.hidden": { "type": "number", "default": 1, "description": "Rarely needed.", "tags": ["advanced"] }
    } } } });
    registry.register_configuration("pub.sample", &contribution);
    registry.finish();
    registered_groups(&registry, Target::User)
}

#[test]
fn the_tree_puts_core_settings_under_their_toc_entry_and_extensions_last() {
    let mut groups = registered_groups(&Registry::core(), Target::User);
    groups.extend(sample_groups());
    let model = Model::new(&groups, false, |id| format!("Name of {id}"));
    let labels: Vec<&str> = model.root.children.iter().map(|group| group.label.as_str()).collect();
    assert_eq!(labels.first(), Some(&"Commonly Used"));
    assert_eq!(labels.last(), Some(&"Extensions"));
    let editor = model.root.children.iter().find(|group| group.id == "editor").unwrap();
    let font = editor.children.iter().find(|group| group.id == "editor/font").unwrap();
    assert!(font.settings.iter().all(|&index| model.settings[index].key.starts_with("editor.font")));
    let extensions = model.root.children.last().unwrap();
    assert_eq!(extensions.children.len(), 1);
    assert_eq!(extensions.children[0].label, "Sample");
    let keys: Vec<&str> = extensions.children[0].settings.iter().map(|&index| model.settings[index].key.as_str()).collect();
    assert_eq!(keys, ["sample.flag", "sample.mode"]);
    let with_advanced = Model::new(&groups, true, |id| id.to_string());
    assert_eq!(with_advanced.root.children.last().unwrap().children[0].settings.len(), 3);
}

#[test]
fn value_types_follow_the_schema() {
    let setting = |key: &str, schema: Value| Setting { key: key.into(), enum_values: schema["enum"].as_array().cloned(), schema, ..Setting::default() };
    assert_eq!(value_type(&setting("a", json!({ "type": "boolean" }))), ValueType::Boolean);
    assert_eq!(value_type(&setting("a", json!({ "type": "string", "enum": ["x"] }))), ValueType::Enum);
    assert_eq!(value_type(&setting("a", json!({ "type": "string", "editPresentation": "multilineText" }))), ValueType::MultilineString);
    assert_eq!(value_type(&setting("files.exclude", json!({ "type": "object" }))), ValueType::Exclude);
    assert_eq!(value_type(&setting("a", json!({ "type": ["integer", "null"] }))), ValueType::NullableInteger);
    assert_eq!(value_type(&setting("a", json!({ "type": "array", "items": { "type": "string" } }))), ValueType::Array);
    assert_eq!(value_type(&setting("a", json!({ "type": "array", "items": { "type": "object" } }))), ValueType::Complex);
    assert_eq!(value_type(&setting("a", json!({ "type": "object", "additionalProperties": { "type": "string" } }))), ValueType::Object);
    assert_eq!(value_type(&setting("a", json!({ "type": "object", "properties": { "x": { "type": "boolean" } }, "additionalProperties": false }))), ValueType::BooleanObject);
    assert_eq!(value_type(&setting("a", json!({ "type": "object" }))), ValueType::Complex);
    assert_eq!(value_type(&setting("[css]", json!({ "type": "object" }))), ValueType::LanguageTag);
}

#[test]
fn local_search_ranks_key_matches_and_falls_back_to_descriptions() {
    let groups = sample_groups();
    let model = Model::new(&groups, true, |id| id.to_string());
    let (matches, exact) = local_search("sample.flag", &model, &groups, |_| None).unwrap();
    assert!(exact);
    let mut matches = matches;
    sort_results(&mut matches, &toc_order(&model), true);
    assert_eq!(model.settings[matches[0].index].key, "sample.flag");
    let (matches, _) = local_search("mode", &model, &groups, |_| None).unwrap();
    assert_eq!(matches.iter().map(|found| model.settings[found.index].key.as_str()).collect::<Vec<_>>(), ["sample.mode"]);
    let (matches, _) = local_search("rarely", &model, &groups, |_| None).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].match_type, match_type::DESCRIPTION_OR_VALUE_MATCH);
    assert!(local_search("  ", &model, &groups, |_| None).is_none());
}

#[test]
fn filters_match_tags_ids_and_extensions() {
    let groups = sample_groups();
    let hidden = groups[0].settings.iter().find(|setting| setting.key == "sample.hidden").unwrap();
    let unset = Inspection::default();
    let configured = Inspection { is_configured: true, ..Inspection::default() };
    let policy = Inspection { has_policy_value: true, ..Inspection::default() };
    assert!(matches_all_tags(hidden, &unset, &strings(&["advanced"])));
    assert!(!matches_all_tags(hidden, &unset, &strings(&["modified"])));
    assert!(matches_all_tags(hidden, &configured, &strings(&["modified", "advanced"])));
    assert!(!matches_all_tags(hidden, &unset, &strings(&["hasPolicy"])));
    assert!(matches_all_tags(hidden, &policy, &strings(&["hasPolicy", "advanced"])));
    assert!(matches_any_id(hidden, &strings(&["sample.*"])));
    assert!(!matches_any_id(hidden, &strings(&["other"])));
    assert!(matches_any_extension(hidden, &strings(&["PUB.sample"])));
    assert!(!should_show_setting(hidden, &parse_query("rarely"), "rarely"));
    assert!(should_show_setting(hidden, &parse_query("@id:sample.hidden"), "@id:sample.hidden"));
}

fn model(json: serde_json::Value) -> config::model::ConfigurationModel {
    config::model::parse_content(&json.to_string(), None, &config::model::ParseFilter::default()).model
}

#[test]
fn inspect_setting_reads_the_selected_scope_and_lists_the_others() {
    let mut configuration = config::configuration::Configuration::default();
    configuration.defaults = model(json!({ "editor.tabSize": 4, "[makefile]": { "editor.tabSize": 8 } }));
    configuration.local_user = model(json!({ "editor.tabSize": 2 }));
    configuration.workspace = model(json!({ "editor.tabSize": 3, "[go]": { "editor.tabSize": 5 } }));
    configuration.folders = Vec::new();
    configuration.has_workspace = true;
    let user = inspect_setting(&configuration, "editor.tabSize", Target::UserLocal, None);
    assert_eq!(user.value, json!(2));
    assert!(user.is_configured);
    assert_eq!(user.default_value, json!(4));
    assert_eq!(user.overridden_scopes, ["workspace:", "workspace:go"]);
    assert_eq!(user.overridden_default_languages, ["makefile"]);
    let workspace = inspect_setting(&configuration, "editor.tabSize", Target::Workspace, None);
    assert_eq!(workspace.value, json!(3));
    assert_eq!(workspace.overridden_scopes, ["user:", "workspace:go"]);
    let unset = inspect_setting(&configuration, "editor.insertSpaces", Target::UserLocal, None);
    assert!(!unset.is_configured);
    assert_eq!(unset.value, serde_json::Value::Null);
}

#[test]
fn a_policy_value_shows_instead_of_the_scope_value_and_is_not_configured() {
    let mut configuration = config::configuration::Configuration::default();
    configuration.defaults = model(json!({ "editor.tabSize": 4 }));
    configuration.local_user = model(json!({ "editor.tabSize": 2 }));
    configuration.policy = model(json!({ "editor.tabSize": 6 }));
    let user = inspect_setting(&configuration, "editor.tabSize", Target::UserLocal, None);
    assert_eq!(user.value, json!(6));
    assert!(user.has_policy_value);
    assert!(!user.is_configured);
    assert!(!inspect_setting(&configuration, "editor.insertSpaces", Target::UserLocal, None).has_policy_value);
}

#[test]
fn markdown_runs_keep_code_and_links_and_drop_their_syntax() {
    let (text, runs) = markdown_runs("Set `afterDelay` in [Files: Auto Save](#files.autoSave \"files.autoSave\") or [x] here.");
    assert_eq!(text, "Set afterDelay in Files: Auto Save or [x] here.");
    assert_eq!(runs, [(4..14, MarkdownRun::Code), (18..34, MarkdownRun::Link("#files.autoSave".into()))]);
    assert_eq!(markdown_runs("a `b").0, "a `b");
}

#[test]
fn setting_links_become_links_named_by_category_and_label() {
    let text = "Refer to the `#search.exclude#` setting or '#files.autoSave#', not `#a b#`.";
    assert_eq!(
        fix_setting_links(text, true),
        "Refer to the [Search: Exclude](#search.exclude \"search.exclude\") setting or [Files: Auto Save](#files.autoSave \"files.autoSave\"), not `#a b#`."
    );
    assert_eq!(fix_setting_links("`#search.exclude#`", false), "\"Search: Exclude\"");
}

#[test]
fn search_suggestions_follow_the_last_word() {
    let all = search_suggestions("", &[], &[]);
    assert_eq!(all[0], "@modified ");
    assert!(all.contains(&"@ext:".to_string()));
    assert!(!search_suggestions("@modified ", &[], &[]).contains(&"@modified ".to_string()));
    assert!(search_suggestions("font", &[], &[]).is_empty());
    assert_eq!(search_suggestions("@lang:", &["rust", "css"], &[]), ["@lang:css ", "@lang:rust "]);
    assert_eq!(search_suggestions("@ext:a.b @ext:", &[], &["a.b", "c.d"]), ["@ext:c.d "]);
}
