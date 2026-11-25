use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::*;

use TerminalCompletionItemKind::{File, Folder};

const PROVIDER: &str = "testProvider";

type Env = Box<dyn Fn(&str) -> Option<String>>;

/// A real folder tree under the temp dir; `path("/test")` is the upstream test's `/test`.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("terminal-completion-{}-{name}", std::process::id()));
        _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create the fixture root");
        Self { root }
    }

    fn path(&self, path: &str) -> String {
        format!("{}{path}", self.root.display())
    }

    fn dir(&self, path: &str) -> &Self {
        fs::create_dir_all(self.path(path)).expect("create a fixture folder");
        self
    }

    fn file(&self, path: &str, executable: bool) -> &Self {
        use std::os::unix::fs::PermissionsExt;
        let path = self.path(path);
        fs::write(&path, "").expect("create a fixture file");
        fs::set_permissions(&path, fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 })).expect("set the fixture file mode");
        self
    }

    fn options(&self, cwd: &str, show_files: bool) -> TerminalCompletionResourceOptions {
        TerminalCompletionResourceOptions { show_files, show_directories: true, glob_pattern: None, cwd: self.path(cwd) }
    }

    fn env(&self, home: &str) -> Env {
        let home = self.path(home);
        Box::new(move |key: &str| (key == "HOME").then(|| home.clone()))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        _ = fs::remove_dir_all(&self.root);
    }
}

type Summary = (String, String, TerminalCompletionItemKind, Option<(usize, usize)>);

fn summarize(completions: &[TerminalCompletion]) -> Vec<Summary> {
    completions.iter().map(|c| (c.label.clone(), c.detail.clone().unwrap_or_default(), c.kind.unwrap_or(Folder), c.replacement_range)).collect()
}

/// `assertCompletions`: exactly these completions in this order.
fn assert_completions(actual: Option<Vec<TerminalCompletion>>, expected: &[(&str, String, TerminalCompletionItemKind)], range: (usize, usize)) {
    let expected: Vec<Summary> = expected.iter().map(|(label, detail, kind)| (label.to_string(), detail.clone(), *kind, Some(range))).collect();
    assert_eq!(summarize(&actual.expect("completions")), expected);
}

/// `assertPartialCompletionsExist`: the first completion with each expected detail matches it.
fn assert_partial_completions_exist(actual: Option<Vec<TerminalCompletion>>, expected: &[(&str, String)], range: (usize, usize)) {
    let actual = summarize(&actual.expect("completions"));
    for (label, detail) in expected {
        let found = actual.iter().find(|summary| summary.1 == *detail);
        assert_eq!(found, Some(&(label.to_string(), detail.clone(), Folder, Some(range))));
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn resolve(options: &TerminalCompletionResourceOptions, prompt: &str, cursor: usize, env: &dyn Fn(&str) -> Option<String>) -> Option<Vec<TerminalCompletion>> {
    resolve_resources(options, prompt, cursor, PROVIDER, None, env, "off")
}

#[test]
fn returns_none_if_neither_show_files_nor_show_directories() {
    let fx = Fixture::new("none");
    fx.dir("/test");
    let options = TerminalCompletionResourceOptions { show_directories: false, ..fx.options("/test", false) };
    assert_eq!(resolve(&options, "cd ", 3, &no_env), None);
}

fn folder_fixture(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    fx.dir("/test/folder1").file("/test/file1.txt", false);
    fx
}

#[test]
fn folders_root_level_completions() {
    let fx = folder_fixture("root-level");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/test", false), "", 1, &fx.env("/home/user")),
        &[(".", d("/test/"), Folder), ("./folder1/", d("/test/folder1/"), Folder), ("../", d("/"), Folder), ("~", d("/home/user/"), Folder)],
        (1, 1),
    );
}

#[test]
fn folders_dot_slash() {
    let fx = folder_fixture("dot-slash");
    let d = |path| fx.path(path);
    assert_completions(resolve(&fx.options("/test", false), "./", 3, &no_env), &[("./", d("/test/"), Folder), ("./folder1/", d("/test/folder1/"), Folder), ("./../", d("/"), Folder)], (1, 3));
}

#[test]
fn folders_parent_folder_completions() {
    let fx = Fixture::new("parent");
    fx.dir("/parent/folder1").dir("/parent/folder2");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/parent/folder1", false), "../", 3, &no_env),
        &[("../", d("/parent/"), Folder), ("../folder1/", d("/parent/folder1/"), Folder), ("../folder2/", d("/parent/folder2/"), Folder), ("../../", d("/"), Folder)],
        (0, 3),
    );
}

#[test]
fn folders_cd_dot_slash() {
    let fx = folder_fixture("cd-dot-slash");
    let d = |path| fx.path(path);
    let expected = [("./", d("/test/"), Folder), ("./folder1/", d("/test/folder1/"), Folder), ("./../", d("/"), Folder)];
    assert_completions(resolve(&fx.options("/test", false), "cd ./", 5, &no_env), &expected, (3, 5));
    assert_completions(resolve(&fx.options("/test", false), "cd ./f", 6, &no_env), &expected, (3, 6));
}

fn hidden_fixture(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    fx.dir("/test/.hiddenFolder").dir("/test/folder1").file("/test/.hiddenFile", true).file("/test/file1.txt", true);
    fx
}

// Children come sorted by name, upstream's mock lists folder1 before file1.txt.
#[test]
fn files_and_folders_hidden() {
    for (prompt, cursor, name) in [("./", 2, "hidden"), ("./h", 3, "hidden-h")] {
        let fx = hidden_fixture(name);
        let d = |path| fx.path(path);
        assert_completions(
            resolve(&fx.options("/test", true), prompt, cursor, &no_env),
            &[
                ("./", d("/test/"), Folder),
                ("./.hiddenFile", d("/test/.hiddenFile"), File),
                ("./.hiddenFolder/", d("/test/.hiddenFolder/"), Folder),
                ("./file1.txt", d("/test/file1.txt"), File),
                ("./folder1/", d("/test/folder1/"), Folder),
                ("./../", d("/"), Folder),
            ],
            (0, cursor),
        );
    }
}

fn home_fixture(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    fx.dir("/test/folder1").dir("/home/vscode/foo").file("/home/vscode/bar.txt", true);
    fx
}

#[test]
fn tilde_completes_to_home() {
    let fx = home_fixture("tilde");
    assert_partial_completions_exist(resolve(&fx.options("/test/folder1", true), "~", 1, &fx.env("/home")), &[("~", fx.path("/home/"))], (0, 1));
}

#[test]
fn tilde_slash_lists_home() {
    let fx = home_fixture("tilde-slash");
    let d = |path| fx.path(path);
    assert_completions(resolve(&fx.options("/test/folder1", true), "~/", 2, &fx.env("/home")), &[("~/", d("/home/"), Folder), ("~/vscode/", d("/home/vscode/"), Folder)], (0, 2));
}

#[test]
fn tilde_folder_lists_its_children() {
    let fx = home_fixture("tilde-folder");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/test/folder1", true), "~/vscode/", 9, &fx.env("/home")),
        &[("~/vscode/", d("/home/vscode/"), Folder), ("~/vscode/bar.txt", d("/home/vscode/bar.txt"), File), ("~/vscode/foo/", d("/home/vscode/foo/"), Folder)],
        (0, 9),
    );
}

#[test]
fn tilde_slash_without_home_is_a_placeholder() {
    let fx = home_fixture("tilde-no-home");
    assert_completions(resolve(&fx.options("/test/folder1", true), "~/", 2, &no_env), &[("~/", "$HOME".into(), Folder)], (0, 2));
}

// The Windows `C:/Foo/` and `c:/foo/` cases are skipped: the port reads POSIX paths only.
#[test]
fn absolute_paths_not_on_windows() {
    let fx = Fixture::new("absolute");
    fx.dir("/foo/Bar").file("/foo/Baz.txt", false);
    let prompt = fx.path("/foo/");
    let cursor = prompt.chars().count();
    assert_completions(resolve(&fx.options("/", false), &prompt, cursor, &no_env), &[(prompt.as_str(), fx.path("/foo/"), Folder), (fx.path("/foo/Bar/").as_str(), fx.path("/foo/Bar/"), Folder)], (0, cursor));
}

// Case-insensitive APFS cannot hold both `FolderA` and `foldera`; the Windows variant is skipped as well.
#[cfg(target_os = "linux")]
#[test]
fn case_sensitivity_should_resolve_correctly_on_unix() {
    let fx = Fixture::new("case");
    fx.dir("/test/FolderA").dir("/test/foldera");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/test", false), "./folder", 8, &no_env),
        &[("./", d("/test/"), Folder), ("./FolderA/", d("/test/FolderA/"), Folder), ("./foldera/", d("/test/foldera/"), Folder), ("./../", d("/"), Folder)],
        (0, 8),
    );
}

fn two_folders(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    fx.dir("/test/folder1").dir("/test/folder2");
    fx
}

#[test]
fn empty_input_resolves_to_the_current_directory() {
    let fx = two_folders("empty");
    let d = |path| fx.path(path);
    let expected = [(".", d("/test/"), Folder), ("./folder1/", d("/test/folder1/"), Folder), ("./folder2/", d("/test/folder2/"), Folder), ("../", d("/"), Folder), ("~", d("/home/user/"), Folder)];
    assert_completions(resolve(&fx.options("/test", false), "", 0, &fx.env("/home/user")), &expected, (0, 0));
    // Must not include the FOO= prefix in completions.
    assert_completions(resolve(&fx.options("/test", false), "FOO=./", 2, &fx.env("/home/user")), &expected, (0, 2));
}

#[test]
fn env_var_prefix_is_ignored_in_the_last_word() {
    let fx = two_folders("env-prefix");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/test", false), "FOO=./", 6, &no_env),
        &[("./", d("/test/"), Folder), ("./folder1/", d("/test/folder1/"), Folder), ("./folder2/", d("/test/folder2/"), Folder), ("./../", d("/"), Folder)],
        (4, 6),
    );
}

#[test]
fn no_completions_when_the_relative_folder_prefix_does_not_exist() {
    let fx = Fixture::new("missing");
    fx.dir("/test/src").dir("/test/vs");
    assert_eq!(resolve(&fx.options("/test", false), "s/", 2, &no_env), None);
}

#[test]
fn large_directories() {
    let fx = Fixture::new("large");
    for index in 0..1000 {
        fx.dir(&format!("/test/folder{index}"));
    }
    let result = resolve(&fx.options("/test", false), "./", 2, &no_env).expect("completions");
    // The 1000 folders, `./` and `./../`.
    assert_eq!(result.len(), 1002);
    assert_eq!(result[0].label, "./");
    assert_eq!(result[1001].label, "./../");
}

#[test]
fn folder_without_trailing_slash_includes_the_current_folder() {
    let fx = two_folders("no-trailing");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/test", false), "./folder1", 10, &no_env),
        &[("./", d("/test/"), Folder), ("./folder1/", d("/test/folder1/"), Folder), ("./folder2/", d("/test/folder2/"), Folder), ("./../", d("/"), Folder)],
        (1, 10),
    );
}

#[test]
fn resolves_a_nested_folder_named_like_the_cwd() {
    let fx = Fixture::new("nested");
    fx.dir("/test/test/inner");
    let d = |path| fx.path(path);
    // `../` of the viewed folder `/test/test/` is `/test/`, not `/`.
    assert_completions(
        resolve(&fx.options("/test", false), "test/", 5, &no_env),
        &[("./test/", d("/test/test/"), Folder), ("./test/inner/", d("/test/test/inner/"), Folder), ("./test/../", d("/test/"), Folder)],
        (0, 5),
    );
}

#[test]
fn dot_slash_cwd_tail_normalizes_to_the_cwd() {
    let fx = two_folders("cwd-tail");
    let d = |path| fx.path(path);
    assert_completions(
        resolve(&fx.options("/test", false), "./test/", 7, &no_env),
        &[("./test/", d("/test/"), Folder), ("./test/folder1/", d("/test/folder1/"), Folder), ("./test/folder2/", d("/test/folder2/"), Folder), ("./test/../", d("/"), Folder)],
        (0, 7),
    );
}

fn cdpath_resolve(fx: &Fixture, cdpath: String, setting: &str) -> Option<Vec<TerminalCompletion>> {
    let env = move |key: &str| (key == "CDPATH").then(|| cdpath.clone());
    resolve_resources(&fx.options("/test", true), "cd ", 3, PROVIDER, None, &env, setting)
}

fn cdpath_fixture(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    fx.dir("/test").dir("/cdpath_value/folder1").file("/cdpath_value/file1.txt", false);
    fx
}

#[test]
fn cdpath_relative() {
    let fx = cdpath_fixture("cdpath-relative");
    let result = cdpath_resolve(&fx, fx.path("/cdpath_value"), "relative");
    assert_partial_completions_exist(result, &[("folder1", format!("CDPATH {}", fx.path("/cdpath_value/folder1/")))], (3, 3));
}

#[test]
fn cdpath_absolute() {
    let fx = cdpath_fixture("cdpath-absolute");
    let result = cdpath_resolve(&fx, fx.path("/cdpath_value"), "absolute");
    assert_partial_completions_exist(result, &[(fx.path("/cdpath_value/folder1/").as_str(), "CDPATH".into())], (3, 3));
}

#[test]
fn cdpath_multiple_paths() {
    let fx = Fixture::new("cdpath-multiple");
    fx.dir("/test").dir("/cdpath1_value/folder1").dir("/cdpath1_value/folder2").file("/cdpath1_value/file1.txt", false);
    fx.dir("/cdpath2_value/inner_dir/folder1").dir("/cdpath2_value/inner_dir/folder2").file("/cdpath2_value/inner_dir/file1.txt", false);
    let result = cdpath_resolve(&fx, format!("{}:{}", fx.path("/cdpath1_value"), fx.path("/cdpath2_value/inner_dir")), "relative");
    let detail = |path| format!("CDPATH {}", fx.path(path));
    assert_partial_completions_exist(
        result,
        &[
            ("folder1", detail("/cdpath1_value/folder1/")),
            ("folder2", detail("/cdpath1_value/folder2/")),
            ("folder1", detail("/cdpath2_value/inner_dir/folder1/")),
            ("folder2", detail("/cdpath2_value/inner_dir/folder2/")),
        ],
        (3, 3),
    );
}

#[test]
fn cdpath_needs_the_setting_and_cd() {
    let fx = cdpath_fixture("cdpath-off");
    let result = cdpath_resolve(&fx, fx.path("/cdpath_value"), "off").expect("completions");
    assert!(result.iter().all(|completion| completion.label != "folder1"));
}

// The gitbash path conversion suite is skipped: it is Windows only.
#[test]
fn symlinks_show_their_target() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new("symlink");
    fx.dir("/test/regular-folder").dir("/target/actual-folder").file("/test/regular-file.txt", false).file("/target/actual-file.txt", false);
    symlink(fx.path("/target/actual-file.txt"), fx.path("/test/symlink-file")).expect("link a file");
    symlink(fx.path("/target/actual-folder"), fx.path("/test/symlink-folder")).expect("link a folder");
    symlink(fx.path("/target/missing"), fx.path("/test/dangling")).expect("link nothing");
    let result = resolve(&fx.options("/test", true), "ls ", 3, &no_env).expect("completions");
    let find = |label: &str| result.iter().find(|completion| completion.label == label).map(|completion| (completion.kind, completion.detail.clone()));
    let real = |path: &str| fs::canonicalize(fx.path(path)).expect("a link target").display().to_string();
    let file_detail = format!("{} -> {}", fx.path("/test/symlink-file"), real("/target/actual-file.txt"));
    let folder_detail = format!("{} -> {}", fx.path("/test/symlink-folder"), real("/target/actual-folder"));
    assert_eq!(find("./symlink-file"), Some((Some(TerminalCompletionItemKind::SymbolicLinkFile), Some(file_detail))));
    assert_eq!(find("./symlink-folder/"), Some((Some(TerminalCompletionItemKind::SymbolicLinkFolder), Some(folder_detail))));
    assert_eq!(find("./dangling"), None);
}

// The remote (WSL) suite is skipped: the port reads the local file system only.
#[test]
fn resource_options_from_dto_take_local_posix_cwds_only() {
    let dto = json!({ "showFiles": true, "cwd": { "scheme": "file", "path": "/work" }, "pathSeparator": "/", "globPattern": "*.ts" });
    let options = TerminalCompletionResourceOptions::from_dto(&dto).expect("local options");
    assert_eq!(options, TerminalCompletionResourceOptions { show_files: true, show_directories: false, glob_pattern: Some(json!("*.ts")), cwd: "/work".into() });
    assert_eq!(TerminalCompletionResourceOptions::from_dto(&json!({ "cwd": { "scheme": "vscode-remote", "path": "/work" }, "pathSeparator": "/" })), None);
    assert_eq!(TerminalCompletionResourceOptions::from_dto(&json!({ "cwd": { "scheme": "file", "path": "/work" }, "pathSeparator": "\\" })), None);
}

#[test]
fn completion_list_from_dto_tags_items_with_the_extension_provider() {
    let dto = json!({
        "items": [{ "label": "ls", "kind": 2, "replacementRange": [0, 2] }, { "kind": 2 }],
        "resourceOptions": { "showDirectories": true, "cwd": { "scheme": "file", "path": "/work" }, "pathSeparator": "/" },
    });
    let list = TerminalCompletionList::from_dto(&dto, "terminal-suggest", "ls");
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].provider, "ext:terminal-suggest");
    assert_eq!(list.resource_options.map(|options| options.cwd), Some("/work".to_owned()));
}

#[test]
fn glob_pattern_filters_files_not_folders() {
    let fx = Fixture::new("glob");
    fx.dir("/test/src").file("/test/a.ts", false).file("/test/b.md", false);
    let options = TerminalCompletionResourceOptions { glob_pattern: Some(json!("**/*.ts")), ..fx.options("/test", true) };
    let labels: Vec<String> = resolve(&options, "ls ", 3, &no_env).expect("completions").into_iter().map(|completion| completion.label).collect();
    assert_eq!(labels, [".", "./a.ts", "./src/", "../", "~"]);
}

#[test]
fn label_escaping_for_posix_shells_without_a_shell_type() {
    let fx = Fixture::new("escaping");
    fx.dir("/test/[folder1]").dir("/test/folder 2").dir("/test/!special$chars&").file("/test/!special$chars2&", true);
    let d = |path| fx.path(path);
    // Children come sorted by name, upstream's mock lists them in creation order.
    assert_completions(
        resolve(&fx.options("/test", true), "", 0, &fx.env("/home/user")),
        &[
            (".", d("/test/"), Folder),
            ("./!special$chars&/", d("/test/!special$chars&/"), Folder),
            ("./!special$chars2&", d("/test/!special$chars2&"), File),
            ("./[folder1]/", d("/test/[folder1]/"), Folder),
            ("./folder 2/", d("/test/folder 2/"), Folder),
            ("../", d("/"), Folder),
            ("~", d("/home/user/"), Folder),
        ],
        (0, 0),
    );
}

#[test]
fn bash_escapes_child_labels() {
    let fx = Fixture::new("escaping-bash");
    fx.dir("/test/[folder1]");
    let result = resolve_resources(&fx.options("/test", false), "cd ", 3, PROVIDER, Some("bash"), &no_env, "off").expect("completions");
    assert_eq!(result[1].label, "./\\[folder1\\]/");
}

#[test]
fn escape_terminal_completion_label_escapes_each_special_char() {
    let cases = [
        ("[abc", "\\[abc"),
        ("abc]", "abc\\]"),
        ("(abc", "\\(abc"),
        ("abc)", "abc\\)"),
        ("'abc", "\\'abc"),
        ("\"abc", "\\\"abc"),
        ("abc\\", "abc\\\\"),
        ("`abc", "\\`abc"),
        ("*abc", "\\*abc"),
        ("?abc", "\\?abc"),
        (";abc", "\\;abc"),
        ("&abc", "\\&abc"),
        ("|abc", "\\|abc"),
        ("<abc", "\\<abc"),
        (">abc", "\\>abc"),
    ];
    for (label, expected) in cases {
        assert_eq!(escape_terminal_completion_label(label, Some("bash")), expected, "{label}");
    }
    assert_eq!(escape_terminal_completion_label("abc", Some("bash")), "abc");
    assert_eq!(escape_terminal_completion_label("[abc", Some("pwsh")), "[abc");
    assert_eq!(escape_terminal_completion_label("[abc", Some("cmd")), "[abc");
}

fn provider(id: &str) -> TerminalCompletionProvider {
    TerminalCompletionProvider { id: id.into(), trigger_characters: vec![], shell_types: None, is_builtin: false }
}

fn enabled_ids(providers: &[TerminalCompletionProvider], config: Value) -> Vec<&str> {
    get_enabled_providers(providers.iter().collect(), &config).into_iter().map(|provider| provider.id.as_str()).collect()
}

#[test]
fn provider_configuration() {
    // Enabled by default when no configuration exists.
    assert_eq!(enabled_ids(&[provider("terminal-suggest"), provider("new-extension-provider")], json!({})), ["terminal-suggest", "new-extension-provider"]);
    // Disabled when explicitly set to false.
    assert_eq!(enabled_ids(&[provider("provider1"), provider("provider2")], json!({ "provider1": false })), ["provider2"]);
    // Enabled when explicitly set to true.
    assert_eq!(enabled_ids(&[provider("provider1"), provider("provider2")], json!({ "provider1": true })), ["provider1", "provider2"]);
    // Mixed configuration.
    assert_eq!(enabled_ids(&[provider("provider1"), provider("provider2"), provider("provider3")], json!({ "provider1": true, "provider2": false })), ["provider1", "provider3"]);
}

#[test]
fn providers_to_request_by_trigger_character_builtin_and_shell() {
    let mut providers = CompletionProviders::default();
    providers.register("ext.a", TerminalCompletionProvider { trigger_characters: vec!["-".into()], ..provider("dash") });
    providers.register("ext.a", TerminalCompletionProvider { is_builtin: true, ..provider("builtin") });
    providers.register("ext.b", TerminalCompletionProvider { shell_types: Some(vec!["pwsh".into()]), ..provider("pwsh-only") });
    let ids = |requested: Vec<&TerminalCompletionProvider>| requested.into_iter().map(|provider| provider.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(providers.providers_to_request("ls -", 4, true, false, None, &json!({}))), ["dash"]);
    assert_eq!(ids(providers.providers_to_request("ls -", 4, false, true, None, &json!({}))), ["builtin"]);
    assert_eq!(ids(providers.providers_to_request("ls ", 3, false, false, Some("zsh"), &json!({ "dash": false }))), ["builtin"]);
    assert_eq!(ids(providers.providers_to_request("ls ", 3, false, false, Some("pwsh"), &json!({}))), ["dash", "builtin", "pwsh-only"]);
    providers.unregister("ext.b", "pwsh-only");
    providers.unregister("ext.a", "dash");
    assert_eq!(ids(providers.providers().collect()), ["builtin"]);
}

#[test]
fn collect_merges_resources_without_duplicate_labels() {
    let fx = Fixture::new("collect");
    fx.dir("/test/src");
    let source = provider("terminal-suggest");
    let items = vec![TerminalCompletion { kind: Some(TerminalCompletionItemKind::Method), replacement_range: Some((0, 0)), ..TerminalCompletion::new("./src/", "") }];
    let list = TerminalCompletionList { items, resource_options: Some(fx.options("/test", false)) };
    let result = collect(vec![(&source, list)], "", 0, Some("pwsh"), &no_env, "off");
    let summary: Vec<_> = result.iter().map(|completion| (completion.label.as_str(), completion.provider.as_str(), completion.is_file_override)).collect();
    assert_eq!(
        summary,
        [("./src/", "", Some(true)), (".", "core:path:ext:terminal-suggest", None), ("../", "core:path:ext:terminal-suggest", None), ("~", "core:path:ext:terminal-suggest", None)]
    );
}
