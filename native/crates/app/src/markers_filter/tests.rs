//! markersFilterOptions.test.ts, and markersTreeViewer.ts `Filter` cases.

use serde_json::json;

use super::*;

fn options(filter: &str) -> FilterOptions {
    FilterOptions::new(filter, FilesExclude::Roots(Vec::new()), true, true, true)
}

fn sources(filter: &str) -> (Vec<String>, Vec<String>, String) {
    let options = options(filter);
    (options.include_source_filters, options.exclude_source_filters, options.text)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn source_filters_are_parsed_out_of_the_text() {
    let cases: [(&str, &[&str], &[&str], &str); 21] = [
        ("@source:ts", &["ts"], &[], ""),
        ("!@source:eslint", &[], &["eslint"], ""),
        ("@source:eslint @source:ts", &["eslint", "ts"], &[], ""),
        ("@source:ts error", &["ts"], &[], "error"),
        ("!@source:ts error", &[], &["ts"], "error"),
        ("some text", &[], &[], "some text"),
        ("@SOURCE:TypeScript", &["typescript"], &[], ""),
        ("text1 @source:eslint @source:ts text2", &["eslint", "ts"], &[], "text1 text2"),
        ("@source:eslint foo", &["eslint"], &[], "foo"),
        ("foo @source:eslint", &["eslint"], &[], "foo"),
        ("foo @source:eslint bar", &["eslint"], &[], "foo bar"),
        ("  @source:eslint foo", &["eslint"], &[], "foo"),
        ("foo @source:eslint  ", &["eslint"], &[], "foo"),
        ("foo !@source:eslint bar", &[], &["eslint"], "foo bar"),
        ("@source:eslint !@source:ts foo", &["eslint"], &["ts"], "foo"),
        ("@source:\"hello world\"", &["hello world"], &[], ""),
        ("@source:\"hello world\" foo", &["hello world"], &[], "foo"),
        ("@source:\"hello world\" @source:eslint @source:ts", &["hello world", "eslint", "ts"], &[], ""),
        ("@source:\"hello world\" @source:\"foo bar\"", &["hello world", "foo bar"], &[], ""),
        ("foo @source:\"hello world\" bar", &["hello world"], &[], "foo bar"),
        ("@source:\"TypeScript Compiler\" @source:eslint !@source:\"My Extension\" text", &["typescript compiler", "eslint"], &["my extension"], "text"),
    ];
    for (filter, include, exclude, text) in cases {
        assert_eq!(sources(filter), (strings(include), strings(exclude), text.to_string()), "{filter}");
    }
}

#[test]
fn markers_match_source_filters_exactly_and_case_insensitively() {
    let cases: [(&str, Option<&str>, bool); 22] = [
        ("foo", Some("eslint"), true),
        ("foo", None, true),
        ("@source:eslint", Some("eslint"), true),
        ("@source:eslint", Some("ESLint"), true),
        ("@source:eslint", Some("ts"), false),
        ("@source:eslint", Some("eslint-plugin"), false),
        ("@source:eslint", Some("es"), false),
        ("@source:eslint", None, false),
        ("!@source:eslint", Some("eslint"), false),
        ("!@source:eslint", Some("ts"), true),
        ("!@source:eslint", Some("eslint-plugin"), true),
        ("!@source:eslint", None, true),
        ("@source:eslint @source:ts", Some("ts"), true),
        ("@source:eslint @source:ts", Some("python"), false),
        ("@source:eslint @source:ts !@source:error", Some("eslint"), true),
        ("@source:eslint @source:ts !@source:error", Some("error"), false),
        ("@source:eslint @source:ts !@source:error", Some("python"), false),
        ("!@source:eslint !@source:ts", Some("ts"), false),
        ("!@source:eslint !@source:ts", Some("python"), true),
        ("!@source:eslint !@source:ts", None, true),
        ("@source:ESLint", Some("EsLiNt"), true),
        ("@source:ESLint", Some("ESLINT"), true),
    ];
    for (filter, source, expected) in cases {
        assert_eq!(options(filter).matches_source_filters(source), expected, "{filter} {source:?}");
    }
}

#[test]
fn files_exclude_exceptions_are_not_excluded() {
    let expression = json!({ "node_modules": true, "!node_modules/@types": true }).as_object().unwrap().clone();
    let options = FilterOptions::new("", FilesExclude::Roots(vec![("/root".to_string(), expression)]), true, true, true);
    assert!(options.excludes.matches("/root/node_modules/react/index.js"));
    assert!(!options.excludes.matches("/root/node_modules/@types/node/index.d.ts"));
}

fn marker(message: &str, severity: u8, source: &str) -> Arc<Marker> {
    Arc::new(Marker::new("owner", "file:///w/a.ts", &json!({ "severity": severity, "message": message, "source": source, "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 2 })).unwrap())
}

fn shown(options: &FilterOptions, path: &str, markers: &[Arc<Marker>]) -> Option<Vec<String>> {
    let name = path.rsplit('/').next().unwrap();
    options.filter_resource(path, name, markers).map(|resource| resource.markers.iter().map(|marker| marker.marker.message.clone()).collect())
}

#[test]
fn text_globs_and_severities_pick_the_shown_markers() {
    let markers = [marker("missing semicolon", ERROR, "ts"), marker("unused variable", WARNING, "eslint"), marker("hint", INFO, "ts")];
    let all = Some(strings(&["missing semicolon", "unused variable", "hint"]));
    assert_eq!(shown(&options(""), "/w/a.ts", &markers), all);
    assert_eq!(shown(&options("unused"), "/w/a.ts", &markers), Some(strings(&["unused variable"])));
    assert_eq!(shown(&options("!unused"), "/w/a.ts", &markers), Some(strings(&["missing semicolon", "hint"])));
    assert_eq!(shown(&options("nothing"), "/w/a.ts", &markers), None);
    // A file name match or an include glob shows every marker of the file.
    assert_eq!(shown(&options("a.ts"), "/w/a.ts", &markers), all);
    assert_eq!(shown(&options(".ts"), "/w/a.ts", &markers), all);
    assert_eq!(shown(&options("!.ts"), "/w/a.ts", &markers), None);
    assert_eq!(shown(&options("@source:ts"), "/w/a.ts", &markers), Some(strings(&["missing semicolon", "hint"])));
    let errors_only = FilterOptions::new("", FilesExclude::Roots(Vec::new()), false, true, false);
    assert_eq!(shown(&errors_only, "/w/a.ts", &markers), Some(strings(&["missing semicolon"])));
}
