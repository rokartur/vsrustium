use serde_json::json;

use super::*;

fn options(filter: &str) -> FilterOptions {
    FilterOptions::new(filter, FilesExclude::ByRoot(Vec::new()), true, true, true)
}

fn sources(filter: &str) -> (Vec<String>, Vec<String>, String) {
    let options = options(filter);
    (options.include_source_filters, options.exclude_source_filters, options.text)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn source_filters_leave_the_rest_as_text() {
    let cases: &[(&str, &[&str], &[&str], &str)] = &[
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
        ("@source:eslint @source:ts foo", &["eslint", "ts"], &[], "foo"),
        ("foo !@source:eslint bar", &[], &["eslint"], "foo bar"),
        ("@source:eslint !@source:ts foo", &["eslint"], &["ts"], "foo"),
        ("@source:\"hello world\"", &["hello world"], &[], ""),
        ("@source:\"hello world\" foo", &["hello world"], &[], "foo"),
        ("@source:\"hello world\" @source:eslint @source:ts", &["hello world", "eslint", "ts"], &[], ""),
        ("@source:\"hello world\" @source:\"foo bar\"", &["hello world", "foo bar"], &[], ""),
        ("!@source:\"hello world\"", &[], &["hello world"], ""),
        ("foo @source:\"hello world\" bar", &["hello world"], &[], "foo bar"),
        ("@source:\"TypeScript Compiler\" @source:eslint !@source:\"My Extension\" text", &["typescript compiler", "eslint"], &["my extension"], "text"),
    ];
    for (filter, include, exclude, text) in cases {
        assert_eq!(sources(filter), (strings(include), strings(exclude), text.to_string()), "{filter}");
    }
}

#[test]
fn source_filters_match_whole_sources_ignoring_case() {
    let check = |filter: &str, expected: &[(Option<&str>, bool)]| {
        let options = options(filter);
        for (source, matches) in expected {
            assert_eq!(options.matches_source_filters(*source), *matches, "{filter} {source:?}");
        }
    };
    check("foo", &[(Some("eslint"), true), (None, true)]);
    check("@source:eslint", &[(Some("eslint"), true), (Some("ESLint"), true), (Some("ts"), false), (Some("eslint-plugin"), false), (Some("es"), false), (None, false)]);
    check("!@source:eslint", &[(Some("eslint"), false), (Some("ts"), true), (Some("eslint-plugin"), true), (None, true)]);
    check("@source:eslint @source:ts", &[(Some("eslint"), true), (Some("ts"), true), (Some("python"), false)]);
    check("@source:eslint @source:ts !@source:error", &[(Some("eslint"), true), (Some("ts"), true), (Some("error"), false), (Some("python"), false)]);
    check("!@source:eslint !@source:ts", &[(Some("eslint"), false), (Some("ts"), false), (Some("python"), true), (None, true)]);
    check("@source:ESLint", &[(Some("eslint"), true), (Some("ESLINT"), true), (Some("EsLiNt"), true)]);
}

#[test]
fn files_exclude_exceptions_are_not_excluded() {
    let expression = json!({ "node_modules": true, "!node_modules/@types": true }).as_object().unwrap().clone();
    let options = FilterOptions::new("", FilesExclude::ByRoot(vec![("/root".into(), expression)]), true, true, true);
    assert_eq!([options.excludes.matches("/root/node_modules/react/index.js"), options.excludes.matches("/root/node_modules/@types/node/index.d.ts")], [true, false]);
}

fn marker(severity: u8, message: &str, source: &str) -> Marker {
    Marker::new("owner", "file:///w/src/a.ts", &json!({ "severity": severity, "message": message, "source": source, "code": "2304", "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 2 })).unwrap()
}

#[test]
fn globs_pick_resources_and_text_picks_markers() {
    let ts = options("*.ts");
    assert_eq!(filter_resource(&ts, "/w/src/a.ts", "a.ts").0, Visibility::Visible);
    assert_eq!(filter_resource(&options("!**/src/**"), "/w/src/a.ts", "a.ts").0, Visibility::Hidden);
    assert_eq!(filter_resource(&options(".js"), "/w/src/a.ts", "a.ts").0, Visibility::Recurse);

    let text = options("name");
    let found = marker(ERROR, "Cannot find name 'x'.", "ts");
    let (visibility, highlights) = filter_marker(&text, &found, Visibility::Recurse);
    assert_eq!((visibility, highlights.lines), (Visibility::Visible, vec![vec![12..16]]));
    assert_eq!(filter_marker(&text, &marker(ERROR, "Unused variable", "ts"), Visibility::Recurse).0, Visibility::Recurse);
    assert_eq!(filter_marker(&options("2304"), &marker(ERROR, "Unused", "ts"), Visibility::Recurse).1.code, vec![0..4]);
}

#[test]
fn negated_text_hides_matches_only_where_the_resource_left_it_open() {
    let negated = options("!name");
    assert_eq!(filter_marker(&negated, &marker(ERROR, "Cannot find name", "ts"), Visibility::Recurse).0, Visibility::Hidden);
    assert_eq!(filter_marker(&negated, &marker(ERROR, "Unused", "ts"), Visibility::Recurse).0, Visibility::Visible);
    assert_eq!(filter_marker(&negated, &marker(ERROR, "Cannot find name", "ts"), Visibility::Visible).0, Visibility::Visible);
}

#[test]
fn severity_toggles_hide_their_markers() {
    let no_warnings = FilterOptions::new("", FilesExclude::ByRoot(Vec::new()), false, true, true);
    assert_eq!(filter_marker(&no_warnings, &marker(WARNING, "w", "ts"), Visibility::Visible).0, Visibility::Hidden);
    assert_eq!(filter_marker(&no_warnings, &marker(ERROR, "e", "ts"), Visibility::Visible).0, Visibility::Visible);
}
