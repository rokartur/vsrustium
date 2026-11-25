use serde_json::json;

use super::*;

fn expression(patterns: &[&str]) -> Option<Expression> {
    Some(patterns.iter().map(|pattern| (pattern.to_string(), Value::Bool(true))).collect())
}

fn global(pattern: &str) -> Vec<String> {
    expand_global_glob(pattern).to_vec()
}

fn search_path(path: &str, patterns: &[&str]) -> SearchPathPattern {
    SearchPathPattern { search_path: PathBuf::from(path), pattern: (!patterns.is_empty()).then(|| expression(patterns).unwrap()) }
}

const ROOT_1: &str = "/foo/root1";

fn single_root() -> Vec<WorkspaceFolder<'static>> {
    vec![WorkspaceFolder { path: Path::new(ROOT_1), name: "root1" }]
}

fn parse(pattern: &str, folders: &[WorkspaceFolder]) -> SearchPathsInfo {
    let workspace = Workspace { folders, single_folder: folders.len() == 1, home: Some("/") };
    // The upstream test turns a thrown error into no search paths.
    parse_search_paths(pattern, &workspace).unwrap_or_default()
}

fn assert_parses(folders: &[WorkspaceFolder], cases: Vec<(&str, SearchPathsInfo)>) {
    for (pattern, expected) in cases {
        assert_eq!(parse(pattern, folders), expected, "{pattern}");
    }
}

fn paths(search_paths: Vec<SearchPathPattern>) -> SearchPathsInfo {
    SearchPathsInfo { search_paths, pattern: None }
}

#[test]
fn simple_includes() {
    let cases = [
        ("a", vec!["**/a/**", "**/a"]),
        ("a/b", vec!["**/a/b", "**/a/b/**"]),
        ("a/b,  c", vec!["**/a/b", "**/c", "**/a/b/**", "**/c/**"]),
        ("a,.txt", vec!["**/a", "**/a/**", "**/*.txt", "**/*.txt/**"]),
        ("a,,,b", vec!["**/a", "**/a/**", "**/b", "**/b/**"]),
        ("**/a,b/**", vec!["**/a", "**/a/**", "**/b/**"]),
    ];
    for (pattern, expected) in cases {
        let result = parse(pattern, &single_root());
        assert_eq!(result.pattern, expression(&expected), "{pattern}");
        assert!(result.search_paths.is_empty());
    }
}

#[test]
fn absolute_includes() {
    let a = global("a");
    assert_parses(
        &single_root(),
        vec![
            ("/foo/bar", paths(vec![search_path("/foo/bar", &[])])),
            ("/foo/bar,a", SearchPathsInfo { search_paths: vec![search_path("/foo/bar", &[])], pattern: expression(&a.iter().map(String::as_str).collect::<Vec<_>>()) }),
            ("/foo/bar,/1/2", paths(vec![search_path("/foo/bar", &[]), search_path("/1/2", &[])])),
            ("/foo/bar,/foo/../foo/bar/fooar/..", paths(vec![search_path("/foo/bar", &[])])),
            ("/foo/bar/**/*.ts", paths(vec![search_path("/foo/bar", &["**/*.ts", "**/*.ts/**"])])),
            ("/foo/bar/*a/b/c", paths(vec![search_path("/foo/bar", &["*a/b/c", "*a/b/c/**"])])),
            ("/*a/b/c", paths(vec![search_path("/", &["*a/b/c", "*a/b/c/**"])])),
            ("/foo/{b,c}ar", paths(vec![search_path("/foo", &["{b,c}ar", "{b,c}ar/**"])])),
        ],
    );
}

#[test]
fn relative_includes_with_a_single_root_folder() {
    assert_parses(
        &single_root(),
        vec![
            ("./a", paths(vec![search_path(ROOT_1, &["a", "a/**"])])),
            ("./a/", paths(vec![search_path(ROOT_1, &["a", "a/**"])])),
            ("./a/*b/c", paths(vec![search_path(ROOT_1, &["a/*b/c", "a/*b/c/**"])])),
            ("./a/*b/c, /project/foo", paths(vec![search_path(ROOT_1, &["a/*b/c", "a/*b/c/**"]), search_path("/project/foo", &[])])),
            ("./a/b/,./c/d", paths(vec![search_path(ROOT_1, &["a/b", "a/b/**", "c/d", "c/d/**"])])),
            ("../", paths(vec![search_path("/foo", &[])])),
            ("..", paths(vec![search_path("/foo", &[])])),
            ("../bar", paths(vec![search_path("/foo/bar", &[])])),
        ],
    );
}

#[test]
fn relative_includes_with_two_root_folders() {
    let folders = [WorkspaceFolder { path: Path::new(ROOT_1), name: "root1" }, WorkspaceFolder { path: Path::new("/project/root2"), name: "root2" }];
    assert_parses(
        &folders,
        vec![
            ("./root1", paths(vec![search_path(ROOT_1, &[])])),
            ("./root2", paths(vec![search_path("/project/root2", &[])])),
            ("./root1/a/**/b, ./root2/**/*.txt", paths(vec![search_path(ROOT_1, &["a/**/b", "a/**/b/**"]), search_path("/project/root2", &["**/*.txt", "**/*.txt/**"])])),
        ],
    );
}

#[test]
fn include_a_named_folder() {
    let folders = [WorkspaceFolder { path: Path::new(ROOT_1), name: "foldername" }, WorkspaceFolder { path: Path::new("/project/root2"), name: "root2" }];
    assert_parses(&folders, vec![("./foldername", paths(vec![search_path(ROOT_1, &[])])), ("./foldername/foo", paths(vec![search_path(ROOT_1, &["foo", "foo/**"])]))]);
}

#[test]
fn folder_with_slash_in_the_name() {
    let folders = [WorkspaceFolder { path: Path::new(ROOT_1), name: "folder/one" }, WorkspaceFolder { path: Path::new("/project/root2"), name: "folder/two+" }];
    assert_parses(
        &folders,
        vec![
            ("./folder/one", paths(vec![search_path(ROOT_1, &[])])),
            ("./folder/two+/foo/", paths(vec![search_path("/project/root2", &["foo", "foo/**"])])),
            ("./folder/onesomethingelse", SearchPathsInfo::default()),
            ("./folder/onesomethingelse/foo", SearchPathsInfo::default()),
            ("./folder", SearchPathsInfo::default()),
        ],
    );
}

#[test]
fn relative_includes_with_ambiguous_root_folders() {
    let folders = [
        WorkspaceFolder { path: Path::new(ROOT_1), name: "root1" },
        WorkspaceFolder { path: Path::new("/project/rootB"), name: "rootB" },
        WorkspaceFolder { path: Path::new("/otherproject/rootB"), name: "rootB" },
    ];
    let both = ["a/**/b", "a/**/b/**", "b/**/*.txt", "b/**/*.txt/**"];
    assert_parses(
        &folders,
        vec![
            ("", SearchPathsInfo::default()),
            ("./", SearchPathsInfo::default()),
            ("./root1", paths(vec![search_path(ROOT_1, &[])])),
            ("./root1,./", paths(vec![search_path(ROOT_1, &[])])),
            ("./rootB", paths(vec![search_path("/project/rootB", &[]), search_path("/otherproject/rootB", &[])])),
            ("./rootB/a/**/b, ./rootB/b/**/*.txt", paths(vec![search_path("/project/rootB", &both), search_path("/otherproject/rootB", &both)])),
            ("./root1/**/foo/, bar/", SearchPathsInfo { search_paths: vec![search_path(ROOT_1, &["**/foo", "**/foo/**"])], pattern: expression(&["**/bar", "**/bar/**"]) }),
        ],
    );
}

#[test]
fn includes_with_tilde() {
    let a = global("a");
    assert_parses(
        &single_root(),
        vec![
            ("~/foo/bar", paths(vec![search_path("/foo/bar", &[])])),
            ("~/foo/bar, a", SearchPathsInfo { search_paths: vec![search_path("/foo/bar", &[])], pattern: expression(&a.iter().map(String::as_str).collect::<Vec<_>>()) }),
            ("/foo/~/bar", paths(vec![search_path("/foo/~/bar", &[])])),
        ],
    );
}

#[test]
fn a_missing_folder_names_the_folder() {
    let folders = [WorkspaceFolder { path: Path::new(ROOT_1), name: "root1" }, WorkspaceFolder { path: Path::new("/project/root2"), name: "root2" }];
    let workspace = Workspace { folders: &folders, single_folder: false, home: None };
    assert_eq!(parse_search_paths("./nope/", &workspace).unwrap_err(), "Workspace folder does not exist: nope/");
}

fn settings(folder: &Path, key: &str) -> Value {
    match (folder.to_str().unwrap(), key) {
        (_, "search.useIgnoreFiles" | "search.useGlobalIgnoreFiles" | "search.useParentIgnoreFiles") => json!(true),
        (ROOT_1, "search.exclude") => json!({ "foo/**/*.js": true }),
        ("/project/root2", "search.exclude") => json!({ "bar": true }),
        _ => Value::Null,
    }
}

fn text(folders: &[WorkspaceFolder], include: &str, exclude: &str) -> Query {
    let workspace = Workspace { folders, single_folder: folders.len() == 1, home: None };
    expanded_text_query(&json!({ "pattern": "a" }), &workspace, include, exclude, &json!({}), &settings).unwrap()
}

#[test]
fn simple_include_with_dot_slash_syntax() {
    let folders = [WorkspaceFolder { path: Path::new("/r"), name: "r" }];
    let query = text(&folders, "bar", "");
    assert_eq!(query.include_pattern, expression(&["**/bar", "**/bar/**"]));
    for include in ["./bar", ".\\bar"] {
        let query = text(&folders, include, "");
        assert_eq!(query.include_pattern, None);
        assert_eq!(query.folder_queries[0].include_pattern, expression(&["bar", "bar/**"]));
    }
}

#[test]
fn exclude_setting_and_search_path() {
    let query = text(&single_root(), "./foo", "");
    let folder = &query.folder_queries[0];
    assert_eq!(folder.folder, Path::new(ROOT_1));
    assert_eq!(folder.include_pattern, expression(&["foo", "foo/**"]));
    assert_eq!(folder.exclude_pattern, expression(&["foo/**/*.js"]));
}

#[test]
fn multiroot_exclude_settings() {
    let folders = [
        WorkspaceFolder { path: Path::new(ROOT_1), name: "root1" },
        WorkspaceFolder { path: Path::new("/project/root2"), name: "root2" },
        WorkspaceFolder { path: Path::new("/project/root3"), name: "root3" },
    ];
    let query = text(&folders, "", "");
    let excludes: Vec<_> = query.folder_queries.iter().map(|folder| folder.exclude_pattern.clone()).collect();
    assert_eq!(excludes, [expression(&["foo/**/*.js"]), expression(&["bar"]), None]);

    let query = text(&folders, "./root2/src", "");
    assert_eq!(query.folder_queries.len(), 1);
    assert_eq!(query.folder_queries[0].folder, Path::new("/project/root2"));
    assert_eq!(query.folder_queries[0].include_pattern, expression(&["src", "src/**"]));
    assert_eq!(query.folder_queries[0].exclude_pattern, expression(&["bar"]));
}

#[test]
fn simple_exclude_input_pattern() {
    let query = text(&single_root(), "", "foo");
    assert_eq!(query.exclude_pattern, expression(&["**/foo/**", "**/foo"]));
}

#[test]
fn exclude_dot_slash_syntax() {
    let folders = [WorkspaceFolder { path: Path::new("/r"), name: "r" }];
    let cases = [("./!old", ["{!}old", "{!}old/**"]), ("./bar", ["bar", "bar/**"]), ("./bar/**/*.ts", ["bar/**/*.ts", "bar/**/*.ts/**"]), (".\\bar\\**\\*.ts", ["bar/**/*.ts", "bar/**/*.ts/**"])];
    for (exclude, expected) in cases {
        let query = text(&folders, "", exclude);
        assert_eq!(query.folder_queries[0].exclude_pattern, expression(&expected), "{exclude}");
    }
}

#[test]
fn a_folder_excluded_whole_is_not_searched() {
    let folders = [WorkspaceFolder { path: Path::new(ROOT_1), name: "root1" }, WorkspaceFolder { path: Path::new("/project/root2"), name: "root2" }];
    let query = text(&folders, "", "./root1");
    assert_eq!(query.folder_queries.iter().map(|folder| folder.folder.clone()).collect::<Vec<_>>(), [PathBuf::from("/project/root2")]);
}

#[test]
fn open_editor_outside_the_folders_passes_include_and_exclude() {
    let folders = [WorkspaceFolder { path: Path::new("/r"), name: "r" }];
    let included = |include: &str, exclude: &str, path: &str| crate::query::path_included_in_query(&text(&folders, include, exclude), Path::new(path));
    assert!(included("", "", "/elsewhere/a.ts"));
    assert!(included("*.ts", "", "/elsewhere/a.ts"));
    assert!(!included("*.js", "", "/elsewhere/a.ts"));
    assert!(!included("", "*.ts", "/elsewhere/a.ts"));
    assert!(included("./src", "", "/r/src/a.ts"));
    assert!(!included("./src", "", "/elsewhere/src/a.ts"));
}

#[test]
fn only_open_editors_searches_the_open_files_under_their_roots() {
    let folders = [WorkspaceFolder { path: Path::new("/r"), name: "r" }];
    let workspace = Workspace { folders: &folders, single_folder: true, home: None };
    let mut query = text(&folders, "", "*.js");
    let open = [PathBuf::from("/r/src/a[1].ts"), PathBuf::from("/r/b.js"), PathBuf::from("/elsewhere/c.ts")];
    restrict_to_open_editors(&mut query, &open, &workspace);
    let roots: Vec<_> = query.folder_queries.iter().map(|folder| (folder.folder.clone(), folder.include_pattern.clone())).collect();
    assert_eq!(roots, [(PathBuf::from("/r"), expression(&["src/a[[]1[]].ts"])), (PathBuf::from("/elsewhere"), expression(&["c.ts"]))]);
    assert_eq!(query.exclude_pattern, None);

    let mut query = text(&folders, "", "*.ts");
    restrict_to_open_editors(&mut query, &open[..1], &workspace);
    assert!(query.folder_queries.is_empty());
    assert_eq!(query.exclude_pattern, expression(&["**/*"]));
}
