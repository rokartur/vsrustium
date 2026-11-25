use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::*;

fn snippet(name: &str, prefix: &str, body: &str, snippet_source: SnippetSource) -> Snippet {
    Snippet {
        is_file_template: false,
        scopes: vec!["fooLang".to_owned()],
        name: name.to_owned(),
        prefix: prefix.to_owned(),
        description: String::new(),
        body: body.to_owned(),
        source: String::new(),
        snippet_source,
        snippet_identifier: format!("{name}/{prefix}"),
        include: None,
        exclude: None,
    }
}

fn user(name: &str, prefix: &str, body: &str) -> Snippet {
    snippet(name, prefix, body, SnippetSource::User)
}

/// `provideCompletionItems` at the 1-based `column` of a one-line model.
fn provide(snippets: &[Snippet], line: &str, column: usize, trigger_character: Option<&str>) -> Vec<Value> {
    provide_with_pairs(snippets, line, column, trigger_character, &[])
}

fn provide_with_pairs(snippets: &[Snippet], line: &str, column: usize, trigger_character: Option<&str>, pairs: &[(String, String)]) -> Vec<Value> {
    let snippets: Vec<&Snippet> = snippets.iter().collect();
    snippet_completions(line, 0, column - 1, trigger_character, pairs, &snippets)["b"].as_array().unwrap().clone()
}

fn label(item: &Value) -> &str {
    item["a"]["label"].as_str().unwrap()
}

fn column(item: &Value, range: &str, end: &str) -> u64 {
    item["j"][range][end].as_u64().unwrap()
}

fn bar_and_bazz() -> Vec<Snippet> {
    vec![user("barTest", "bar", "barCodeSnippet"), user("bazzTest", "bazz", "bazzCodeSnippet")]
}

#[test]
fn snippet_completions_simple() {
    assert_eq!(provide(&bar_and_bazz(), "", 1, None).len(), 2);
}

#[test]
fn snippet_completions_simple_2() {
    assert_eq!(provide(&bar_and_bazz(), "hello ", 6, None).len(), 0);
    assert_eq!(provide(&bar_and_bazz(), "hello ", 7, None).len(), 2);
}

#[test]
fn snippet_completions_with_prefix() {
    let result = provide(&bar_and_bazz(), "bar", 4, None);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["a"], json!({ "label": "bar", "description": "barTest" }));
    assert_eq!(column(&result[0], "insert", "startColumn"), 1);
    assert_eq!(result[0]["h"], "barCodeSnippet");
}

#[test]
fn snippet_completions_with_different_prefixes() {
    let snippets = [user("barTest", "bar", "s1"), user("name", "bar-bar", "s2")];
    for (position, bar_start) in [(3, 1), (5, 5), (6, 5)] {
        let result = provide(&snippets, "bar-bar", position, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["a"], json!({ "label": "bar", "description": "barTest" }));
        assert_eq!(result[0]["h"], "s1");
        assert_eq!(column(&result[0], "insert", "startColumn"), bar_start);
        assert_eq!(result[1]["a"], json!({ "label": "bar-bar", "description": "name" }));
        assert_eq!(result[1]["h"], "s2");
        assert_eq!(column(&result[1], "insert", "startColumn"), 1);
    }
}

#[test]
fn cannot_use_php_as_user_snippet_prefix_anymore_26275() {
    let snippets = [user("", "<?php", "insert me")];
    assert_eq!(provide(&snippets, "\t<?php", 7, None).len(), 1);
    for line in ["\t<?", "a<?"] {
        let result = provide(&snippets, line, 4, None);
        assert_eq!(result.len(), 1);
        assert_eq!(column(&result[0], "insert", "startColumn"), 2);
    }
}

#[test]
fn no_user_snippets_in_suggestions_when_inside_the_code_30508() {
    let snippets = [user("", "foo", "<foo>$0</foo>")];
    assert_eq!(provide(&snippets, "<head>", 1, None).len(), 1);
    assert_eq!(provide(&snippets, "\t", 2, None).len(), 1);
}

#[test]
fn snippet_suggest_ensure_extension_snippets_come_last() {
    let snippets = [snippet("second", "second", "second", SnippetSource::Extension), user("first", "first", "first")];
    let result = provide(&snippets, "", 1, None);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["a"], json!({ "label": "first", "description": "first" }));
    assert_eq!(result[1]["a"], json!({ "label": "second", "description": "second" }));
    assert_eq!((result[0]["e"].as_str(), result[1]["e"].as_str()), (Some("a-first"), Some("z-second")));
}

#[test]
fn dash_in_snippets_prefix_broken_53945() {
    let snippets = [user("p-a", "p-a", "second")];
    assert_eq!(provide(&snippets, "p-", 2, None).len(), 1);
    assert_eq!(provide(&snippets, "p-", 3, None).len(), 1);
}

#[test]
fn no_snippets_suggestion_on_long_lines_beyond_character_100_58807() {
    let line = "Thisisaverylonglinegoingwithmore100bcharactersandthismakesintellisensebecomea Thisisaverylonglinegoingwithmore100bcharactersandthismakesintellisensebecomea b";
    assert_eq!(provide(&[user("bug", "bug", "second")], line, 158, None).len(), 1);
}

#[test]
fn type_colon_will_trigger_snippet_60746() {
    assert_eq!(provide(&[user("bug", "bug", "second")], ":", 2, None).len(), 0);
}

#[test]
fn substring_of_prefix_cant_trigger_snippet_60737() {
    let result = provide(&[user("mytemplate", "mytemplate", "second")], "template", 9, None);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["a"], json!({ "label": "mytemplate", "description": "mytemplate" }));
}

#[test]
fn no_snippets_suggestion_beyond_character_100_if_not_at_end_of_line_60247() {
    let line = "Thisisaverylonglinegoingwithmore100bcharactersandthismakesintellisensebecomea Thisisaverylonglinegoingwithmore100bcharactersandthismakesintellisensebecomea b text_after_b";
    assert_eq!(provide(&[user("bug", "bug", "second")], line, 158, None).len(), 1);
}

#[test]
fn issue_61296_vs_code_freezes_when_editing_css_file_with_emoji() {
    // Upstream's position is UTF-16 column 8; the pig is one char here, so the end of the line is char column 7.
    let result = provide(&[user("bug", "-a-bug", "second")], ".🐷-a-b", 7, None);
    assert_eq!(result.len(), 1);
    assert_eq!(column(&result[0], "insert", "startColumn"), 4);
}

#[test]
fn no_snippets_shown_when_triggering_completions_at_whitespace_on_line_that_already_has_text_62335() {
    assert_eq!(provide(&[user("bug", "bug", "second")], "a ", 3, None).len(), 1);
}

#[test]
fn snippet_prefix_with_special_chars_and_numbers_does_not_work_62906() {
    let snippets = [user("noblockwdelay", "<<", "<= #dly\""), user("noblockwdelay", "11", "eleven")];
    let result = provide(&snippets, " <", 3, None);
    assert_eq!(result.len(), 1);
    assert_eq!(column(&result[0], "insert", "startColumn"), 2);
    let result = provide(&snippets, "1", 2, None);
    assert_eq!(result.len(), 1);
    assert_eq!(column(&result[0], "insert", "startColumn"), 1);
}

#[test]
fn snippet_replace_range() {
    let snippets = [user("notWordTest", "not word", "not word snippet")];
    for (line, position, insert_end, replace_end) in [("not wordFoo bar", 3, 3, 9), ("not woFoo bar", 3, 3, 3), ("not word", 1, 1, 9)] {
        let result = provide(&snippets, line, position, None);
        assert_eq!(result.len(), 1);
        assert_eq!(column(&result[0], "insert", "endColumn"), insert_end);
        assert_eq!(column(&result[0], "replace", "endColumn"), replace_end);
    }
}

#[test]
fn snippet_replace_range_incorrect_108894() {
    let result = provide(&[user("eng", "eng", "<span></span>")], "filler e KEEP ng filler", 9, None);
    assert_eq!(result.len(), 1);
    assert_eq!(column(&result[0], "insert", "endColumn"), 9);
    assert_eq!(column(&result[0], "replace", "endColumn"), 9);
}

#[test]
fn snippet_will_replace_auto_closing_pair_if_specified_in_prefix() {
    let pairs: Vec<(String, String)> = [("{", "}"), ("[", "]"), ("(", ")")].iter().map(|(open, close)| (open.to_string(), close.to_string())).collect();
    let snippets = [user("PSCustomObject", "[PSCustomObject]", "[PSCustomObject] @{ Key = Value }")];
    let result = provide_with_pairs(&snippets, "[psc]", 5, None, &pairs);
    assert_eq!(result.len(), 1);
    assert_eq!(column(&result[0], "insert", "endColumn"), 5);
    assert_eq!(column(&result[0], "replace", "endColumn"), 6);
}

#[test]
fn leading_whitespace_in_snippet_prefix_123860() {
    let result = provide(&[user("cite-name", " cite", "~\\cite{$CLIPBOARD}")], " ci", 4, None);
    assert_eq!(result.len(), 1);
    assert_eq!(label(&result[0]), " cite");
    assert_eq!(column(&result[0], "insert", "startColumn"), 1);
}

#[test]
fn still_show_suggestions_in_string_when_disable_string_suggestion_136611() {
    let snippets = [user("aaa", "aaa", "value"), user("bbb", "bbb", "value")];
    assert_eq!(provide(&snippets, "''", 2, Some("'")).len(), 0);
}

#[test]
fn still_show_suggestions_in_string_when_disable_string_suggestion_136611_part_2() {
    let snippets = [user("aaa", "aaa", "value"), user("bbb", "bbb", "value"), user("'ccc", "'ccc", "value")];
    assert_eq!(provide(&snippets, "''", 2, Some("'")).len(), 1);
}

#[test]
fn snippet_suggestions_are_too_eager_138707_word() {
    let snippets = [user("tys", "tys", "value"), user("hell_or_tell", "hell_or_tell", "value"), user("^y", "^y", "value")];
    let result = provide(&snippets, "'hellot'", 8, None);
    assert_eq!(result.len(), 1);
    assert_eq!(label(&result[0]), "hell_or_tell");
}

#[test]
fn snippet_suggestions_are_too_eager_138707_no_word() {
    let snippets = [user("tys", "tys", "value"), user("t", "t", "value"), user("^y", "^y", "value")];
    let result = provide(&snippets, ")*&^", 5, None);
    assert_eq!(result.len(), 1);
    assert_eq!(label(&result[0]), "^y");
}

#[test]
fn snippet_suggestions_are_too_eager_138707_word_word() {
    let snippets = [user("async arrow function", "async arrow function", "value"), user("foobarrrrrr", "foobarrrrrr", "value")];
    let result = provide(&snippets, "foobar", 7, None);
    assert_eq!(result.len(), 1);
    assert_eq!(label(&result[0]), "foobarrrrrr");
}

#[test]
fn strange_and_useless_autosuggestion_region_endregion_php_140039() {
    assert_eq!(provide(&[user("reg", "#region", "value")], "function abc(w)", 15, None).len(), 0);
}

#[test]
fn hyphen_in_snippet_prefix_de_indents_snippet_139016() {
    let result = provide(&[user("foo", "Foo- Bar", "Foo")], "    bar", 8, None);
    assert_eq!(result.len(), 1);
    assert_eq!(column(&result[0], "insert", "startColumn"), 5);
}

#[test]
fn autocomplete_suggests_based_on_the_last_letter_of_a_word_and_it_depends_on_the_typing_speed_191070() {
    let snippets = [user("/whiletrue", "/whiletrue", "one"), user("/sc not expanding", "/sc not expanding", "two")];
    for (line, position) in [("w", 2), ("where", 6)] {
        let result = provide(&snippets, line, position, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["h"], "one");
    }
}

#[test]
fn snippet_completion_carries_detail_kind_rules_and_command() {
    let mut with_description = user("name", "pre", "body");
    with_description.description = "says what".to_owned();
    with_description.source = "User Snippet".to_owned();
    let result = provide(&[with_description], "", 1, None);
    assert_eq!(result[0]["c"], "says what (User Snippet)");
    assert_eq!((result[0]["b"].as_u64(), result[0]["i"].as_u64()), (Some(KIND_SNIPPET), Some(INSERT_AS_SNIPPET)));
    assert_eq!((result[0]["o"].as_str(), &result[0]["p"]), (Some("_snippet.markAsUsed"), &json!(["name/pre"])));
}

#[test]
fn resolve_documents_the_inserted_text() {
    let resolved = resolve_snippet_completion(&user("name", "pre", "for ${1:item} of $LIST {$0}"));
    assert_eq!(resolved["d"]["value"], "\n```\nfor item of  {}\n```\n");
    let resolved = resolve_snippet_completion(&user("name", "pre", "```js"));
    assert_eq!(resolved["d"]["value"], "\n````\n```js\n````\n");
}

fn included(include: Option<&[&str]>, exclude: Option<&[&str]>, path: &str) -> bool {
    let mut snippet = user("name", "prefix", "snippet");
    snippet.include = include.map(|patterns| patterns.iter().map(|pattern| pattern.to_string()).collect());
    snippet.exclude = exclude.map(|patterns| patterns.iter().map(|pattern| pattern.to_string()).collect());
    snippet.is_file_included(path)
}

#[test]
fn get_snippets_sync_include_pattern() {
    let test = Some(&["**/*.test.ts"][..]);
    assert!(included(test, None, "/project/src/foo.test.ts"));
    assert!(!included(test, None, "/project/src/foo.spec.ts"));
    assert!(!included(test, None, "/project/src/foo.ts"));
    assert!(included(None, None, "/project/src/foo.ts"));
}

#[test]
fn get_snippets_sync_exclude_pattern() {
    let prod = Some(&["**/*.min.js", "**/dist/**"][..]);
    assert!(included(None, prod, "/project/src/foo.js"));
    assert!(!included(None, prod, "/project/src/foo.min.js"));
    assert!(!included(None, prod, "/project/dist/bundle.js"));
}

#[test]
fn get_snippets_sync_include_and_exclude_patterns_together() {
    let (include, exclude) = (Some(&["**/*.test.ts", "**/*.spec.ts"][..]), Some(&["**/*.perf.test.ts"][..]));
    assert!(included(include, exclude, "/project/src/foo.test.ts"));
    assert!(included(include, exclude, "/project/src/foo.spec.ts"));
    assert!(!included(include, exclude, "/project/src/foo.perf.test.ts"));
    assert!(!included(include, exclude, "/project/src/foo.ts"));
}

#[test]
fn get_snippets_sync_filename_only_patterns_no_path_separator() {
    assert!(included(Some(&["*.test.ts"]), None, "/project/src/foo.test.ts"));
    assert!(included(Some(&["*.test.ts"]), None, "/other/deep/path/bar.test.ts"));
    assert!(included(Some(&["config.json"]), None, "/project/config.json"));
    assert!(included(Some(&["config.json"]), None, "/deep/nested/path/config.json"));
    assert!(!included(Some(&["config.json"]), None, "/project/myconfig.json"));
}

fn code_snippets_file(data: Vec<Snippet>) -> SnippetFile {
    SnippetFile { source: SnippetSource::Extension, location: "somepath/foo.code-snippets".into(), default_scopes: None, is_global_snippets: true, is_user_snippets: false, data }
}

fn select_count(file: &SnippetFile, selector: &str) -> usize {
    let mut bucket = Vec::new();
    file.select(selector, &mut bucket);
    bucket.len()
}

fn scoped(scopes: &[&str], name: &str) -> Snippet {
    Snippet { scopes: scopes.iter().map(|scope| scope.to_string()).collect(), ..user(name, "foo", "snippet") }
}

#[test]
fn snippet_file_select() {
    assert_eq!(select_count(&code_snippets_file(Vec::new()), ""), 0);
    let file = code_snippets_file(vec![
        scoped(&["foo"], "FooSnippet1"),
        scoped(&["foo"], "FooSnippet2"),
        scoped(&["bar"], "BarSnippet1"),
        scoped(&["bar.comment"], "BarSnippet2"),
        scoped(&["bar.strings"], "BarSnippet2"),
        scoped(&["bazz", "bazz"], "BazzSnippet1"),
    ]);
    assert_eq!(select_count(&file, "foo"), 2);
    assert_eq!(select_count(&file, "fo"), 0);
    assert_eq!(select_count(&file, "bar"), 1);
    assert_eq!(select_count(&file, "bar.comment"), 2);
    assert_eq!(select_count(&file, "bazz"), 1);
}

#[test]
fn snippet_file_select_any_scope() {
    let file = code_snippets_file(vec![scoped(&[], "AnySnippet1"), scoped(&["foo"], "FooSnippet1")]);
    assert_eq!(select_count(&file, "foo"), 2);
}

#[test]
fn parse_reads_groups_prefix_arrays_and_scopes() {
    let text = r#"{
        // a comment
        "For": { "prefix": ["for", "fr"], "body": ["a", "b"], "description": "loop", "scope": "js, ts ,", },
        "Group": { "Inner": { "prefix": "in", "body": "x", "include": "*.ts" } },
        "No body": { "prefix": "nb" },
        "Numbers": 5,
    }"#;
    let file = SnippetFile::parse(SnippetSource::User, Path::new("/u/snippets/my.code-snippets"), None, None, text);
    let summary: Vec<_> = file.data.iter().map(|s| (s.name.as_str(), s.prefix.as_str(), s.body.as_str(), s.scopes.join("|"), s.source.as_str(), s.snippet_identifier.as_str())).collect();
    assert_eq!(
        summary,
        [
            ("For", "for", "a\nb", "js|ts".to_owned(), "Global User Snippet", "my.code-snippets/For"),
            ("For", "fr", "a\nb", "js|ts".to_owned(), "Global User Snippet", "my.code-snippets/For"),
            ("Inner", "in", "x", String::new(), "Global User Snippet", "my.code-snippets/Inner"),
        ]
    );
    assert_eq!(file.data[2].include, Some(vec!["*.ts".to_owned()]));

    let language_file = SnippetFile::parse(SnippetSource::User, Path::new("/u/snippets/rust.json"), Some(vec!["rust".into()]), None, text);
    assert_eq!((select_count(&language_file, "rust"), select_count(&language_file, "js")), (3, 0));
    assert_eq!(language_file.data[0].source, "User Snippet");

    let extension_file = SnippetFile::parse(SnippetSource::Extension, Path::new("/ext/snippets/a.json"), None, Some(("Ext", Path::new("/ext"))), text);
    assert_eq!((extension_file.data[0].source.as_str(), extension_file.data[0].snippet_identifier.as_str()), ("Ext", "snippets/a.json/For"));

    let broken = SnippetFile::parse(SnippetSource::User, Path::new("/u/b.code-snippets"), None, None, r#"{ "a": { "prefix": "a", "body": "x" }, "b": null }"#);
    assert!(broken.data.is_empty());
}

#[test]
fn extension_contributions_are_validated_and_merged() {
    let mut files = Vec::new();
    let contributions = [
        json!({ "language": "rust", "path": "./snippets/a.json" }),
        json!({ "language": "toml", "path": "snippets/a.json" }),
        json!({ "path": "./b.json" }),
        json!({ "language": "nope", "path": "./c.json" }),
        json!({ "language": "rust", "path": "../outside.json" }),
        json!({ "path": "./d.code-snippets" }),
        json!({ "language": "rust", "path": "./d.code-snippets" }),
        json!({ "language": "toml", "path": "./d.code-snippets" }),
    ];
    let errors = add_extension_snippet_files(&mut files, "ext", Path::new("/x/ext"), &contributions, |language| language == "rust" || language == "toml");
    assert_eq!(
        files,
        [
            SnippetFileLocation { location: "/x/ext/snippets/a.json".into(), default_scopes: Some(vec!["rust".into(), "toml".into()]) },
            SnippetFileLocation { location: "/x/ext/d.code-snippets".into(), default_scopes: Some(vec!["toml".into()]) },
        ]
    );
    assert_eq!(
        errors,
        [
            "When omitting the language, the value of `contributes.ext.path` must be a `.code-snippets`-file. Provided value: ./b.json",
            "Unknown language in `contributes.ext.language`. Provided value: nope",
            "Expected `contributes.ext.path` (/x/outside.json) to be included inside extension's folder (/x/ext). This might make the extension non-portable.",
        ]
    );
}

#[test]
fn folder_files_get_their_language_scope() {
    let user_json = folder_snippet_file(Path::new("/u/rust.json"), SnippetSource::User);
    assert_eq!(user_json.and_then(|file| file.default_scopes), Some(vec!["rust".to_owned()]));
    assert_eq!(folder_snippet_file(Path::new("/w/.vscode/a.code-snippets"), SnippetSource::Workspace).map(|file| file.default_scopes), Some(None));
    assert_eq!(folder_snippet_file(Path::new("/w/.vscode/settings.json"), SnippetSource::Workspace), None);
}

#[test]
fn filter_and_sort_drops_no_prefix_and_disabled_then_sorts_by_recency() {
    let snippets = [snippet("ext", "e", "", SnippetSource::Extension), user("b", "b", ""), user("a", "a", ""), user("none", "", ""), user("off", "off", "")];
    let ignored = HashSet::from(["off/off".to_owned()]);
    let usage = HashMap::from([("b/b".to_owned(), 10)]);
    let sorted = filter_and_sort_snippets(snippets.iter().collect(), None, SnippetGetOptions::default(), &ignored, &usage);
    assert_eq!(sorted.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b", "a", "ext"]);
    let options = SnippetGetOptions { include_no_prefix_snippets: true, include_disabled_snippets: true, no_recency_sort: true, ..SnippetGetOptions::default() };
    let sorted = filter_and_sort_snippets(snippets.iter().collect(), None, options, &ignored, &usage);
    assert_eq!(sorted.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a", "b", "none", "off", "ext"]);
}
