//! extensionQuery.test.ts and the local query cases of extensionsViews.test.ts.

use exthost::scan::ScannedExtension;
use serde_json::{Value, json};

use super::*;

#[test]
fn parse() {
    let cases = [
        ("", "", ""),
        ("hello", "hello", ""),
        ("   hello world ", "hello world", ""),
        ("@sort", "@sort", ""),
        ("@sort:", "@sort:", ""),
        ("  @sort:  ", "@sort:", ""),
        ("@sort:installs", "", "installs"),
        ("   @sort:installs   ", "", "installs"),
        ("@sort:installs-", "", "installs"),
        ("@sort:installs-foo", "", "installs"),
        ("vs @sort:installs", "vs", "installs"),
        ("vs @sort:installs code", "vs  code", "installs"),
        ("@sort:installs @sort:ratings", "", "ratings"),
    ];
    for (input, value, sort_by) in cases {
        assert_eq!(Query::parse(input), Query::new(value, sort_by), "{input:?}");
    }
}

#[test]
fn to_string() {
    assert_eq!(Query::new("hello", "").to_string(), "hello");
    assert_eq!(Query::new("  hello    ", "").to_string(), "hello");
    assert_eq!(Query::new("", "installs").to_string(), "@sort:installs");
    assert_eq!(Query::new("  hello      ", "installs").to_string(), "hello @sort:installs");
}

#[test]
fn query_types() {
    assert!(is_built_in_extensions_query("@builtin"));
    for query in ["@installed", "@enabled", "@disabled", "@outdated", "@updates", "@sort:name", "@sort:updateDate", "@installed searchText", "@enabled searchText", "@disabled searchText", "@outdated searchText", "@updates searchText"] {
        assert!(is_local_extensions_query(query, ""), "{query}");
    }
    for query in ["@agentPlugins @installed", "@agentPlugins @installed searchText", "@mcp @installed", "@mcp @installed searchText"] {
        assert!(!is_local_extensions_query(query, ""), "{query}");
    }
}

fn local(name: &str, manifest: Value, builtin: bool, installed: u64, running: bool) -> Extension {
    let mut description = json!({ "name": name, "publisher": "pub", "version": "1.0.0", "isBuiltin": builtin, "__metadata": { "installedTimestamp": installed } });
    for (key, value) in manifest.as_object().unwrap() {
        description[key] = value.clone();
    }
    let local = ScannedExtension { id: format!("pub.{name}"), location: Default::default(), description };
    let enablement = if running { EnablementState::EnabledGlobally } else { EnablementState::DisabledGlobally };
    Extension { local: Some(local), gallery: None, state: ExtensionState::Installed, enablement, running, restart_required: false }
}

const NOW: u64 = 1_000_000_000_000;

/// The installed extensions of extensionsViews.test.ts.
fn fixture() -> Vec<Extension> {
    vec![
        local("first-enabled-extension", json!({ "categories": ["Themes", "random"] }), false, 123456, true),
        local("second-enabled-extension", json!({ "categories": ["Programming languages"] }), false, NOW, true),
        local("random-enabled-extension", json!({ "categories": ["random"] }), false, 345678, true),
        local("first-disabled-extension", json!({ "categories": ["themes"] }), false, 234567, false),
        local("second-disabled-extension", json!({ "categories": ["programming languages"], "__metadata": { "installedTimestamp": NOW - 50000, "updated": true } }), false, 0, false),
        local("my-theme", json!({ "categories": ["Themes"], "contributes": { "themes": ["my-theme"] } }), true, 222, true),
        local("my-lang", json!({ "categories": ["Programming Languages"], "contributes": { "grammars": [{ "language": "my-language" }] } }), true, 666666, true),
    ]
}

fn names(query: &str) -> Vec<String> {
    let extensions = fixture();
    let query = Query::parse(query);
    filter_local(&extensions, &query, SortBy::parse(&query.sort_by), NOW).iter().map(|extension| extension.name().to_owned()).collect()
}

#[test]
fn installed_query_results() {
    assert_eq!(names("@installed"), ["first-enabled-extension", "random-enabled-extension", "second-enabled-extension", "first-disabled-extension", "second-disabled-extension"]);
    assert_eq!(names("@installed first"), ["first-enabled-extension", "first-disabled-extension"]);
    assert_eq!(names("@disabled"), ["first-disabled-extension", "second-disabled-extension"]);
    assert_eq!(names("@enabled"), ["first-enabled-extension", "random-enabled-extension", "second-enabled-extension"]);
    assert_eq!(names("@builtin category:themes"), ["my-theme"]);
    assert_eq!(names("@builtin category:\"programming languages\""), ["my-lang"]);
    assert_eq!(names("@builtin"), ["my-lang", "my-theme"]);
    assert_eq!(names("@builtin my-theme"), ["my-theme"]);
}

#[test]
fn installed_query_with_category() {
    assert_eq!(names("@installed category:themes"), ["first-enabled-extension", "first-disabled-extension"]);
    assert_eq!(names("@installed category:\"themes\""), ["first-enabled-extension", "first-disabled-extension"]);
    assert_eq!(names("@installed category:\"programming languages\""), ["second-enabled-extension", "second-disabled-extension"]);
    assert_eq!(names("@installed category:themes category:random"), ["first-enabled-extension", "random-enabled-extension", "first-disabled-extension"]);
    assert_eq!(names("@enabled category:themes"), ["first-enabled-extension"]);
    assert_eq!(names("@enabled category:\"programming languages\""), ["second-enabled-extension"]);
    assert_eq!(names("@disabled category:themes"), ["first-disabled-extension"]);
    assert_eq!(names("@disabled category:\"programming languages\""), ["second-disabled-extension"]);
}

#[test]
fn local_query_with_sorting_order() {
    assert_eq!(names("@recentlyUpdated"), ["second-disabled-extension"]);
    assert_eq!(names("@installed @sort:updateDate"), ["second-enabled-extension", "second-disabled-extension", "random-enabled-extension", "first-disabled-extension", "first-enabled-extension"]);
}

#[test]
fn contribute_lists_extensions_with_the_feature() {
    assert_eq!(names("@contribute:themes"), ["my-theme"]);
    assert!(names("@contribute:unknown").is_empty());
}
