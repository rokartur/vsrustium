use super::super::completion_item::TerminalCompletion;
use super::*;

use TerminalCompletionItemKind::*;

fn create_item(label: &str, kind: TerminalCompletionItemKind) -> TerminalCompletionItem {
    TerminalCompletionItem::new(TerminalCompletion { kind: Some(kind), replacement_range: Some((0, 1)), ..TerminalCompletion::new(label, "defaultProvider") }, None)
}

fn method(label: &str) -> TerminalCompletionItem {
    create_item(label, Method)
}

fn items_of(labels: &[&str], kind: TerminalCompletionItemKind) -> Vec<TerminalCompletionItem> {
    labels.iter().map(|label| create_item(label, kind)).collect()
}

fn labels(model: &mut TerminalCompletionModel) -> Vec<String> {
    model.items().iter().map(|item| item.completion.label.clone()).collect()
}

fn assert_items(items: Vec<TerminalCompletionItem>, line: &str, expected: &[&str]) {
    assert_eq!(labels(&mut TerminalCompletionModel::new(items, LineContext::new(line, 0))), expected);
}

#[test]
fn should_handle_an_empty_list() {
    assert_eq!(TerminalCompletionModel::new(vec![], LineContext::new("", 0)).items().len(), 0);
}

#[test]
fn should_handle_a_list_with_one_item() {
    assert_items(vec![method("a")], "", &["a"]);
}

#[test]
fn should_sort_alphabetically() {
    assert_items(vec![method("b"), method("z"), method("a")], "", &["a", "b", "z"]);
}

#[test]
fn fuzzy_matching() {
    let initial = [".\\.eslintrc", ".\\resources\\", ".\\scripts\\", ".\\src\\"];
    assert_items(initial.map(method).to_vec(), "s", &[".\\scripts\\", ".\\src\\", ".\\.eslintrc", ".\\resources\\"]);
}

#[test]
fn files_and_folders_should_deprioritize_files_that_start_with_underscore() {
    let initial = ["_a", "a", "z"];
    assert_items(items_of(&initial, File), "", &["a", "z", "_a"]);
    assert_items(items_of(&initial, Folder), "", &["a", "z", "_a"]);
}

#[test]
fn files_and_folders_should_ignore_the_dot_in_dotfiles_when_sorting() {
    let initial = ["b", ".a", "a", ".b"];
    assert_items(items_of(&initial, File), "", &[".a", "a", "b", ".b"]);
    assert_items(items_of(&initial, Folder), "", &[".a", "a", "b", ".b"]);
}

#[test]
fn files_and_folders_should_handle_many_files_and_folders_correctly() {
    // VS Code's root directory with some python items that have special sorting.
    let mut items = items_of(
        &[
            "__pycache", ".build", ".configurations", ".devcontainer", ".eslint-plugin-local", ".github", ".profile-oss", ".vscode", ".vscode-test", "build", "cli",
            "extensions", "node_modules", "out", "remote", "resources", "scripts", "src", "test",
        ],
        Folder,
    );
    items.extend(items_of(
        &[
            "__init__.py", ".editorconfig", ".eslint-ignore", ".git-blame-ignore-revs", ".gitattributes", ".gitignore", ".lsifrc.json", ".mailmap", ".mention-bot", ".npmrc",
            ".nvmrc", ".vscode-test.js", "cglicenses.json", "cgmanifest.json", "CodeQL.yml", "CONTRIBUTING.md", "eslint.config.js", "gulpfile.js", "LICENSE.txt",
            "package-lock.json", "package.json", "product.json", "README.md", "SECURITY.md", "ThirdPartyNotices.txt", "tsfmt.json",
        ],
        File,
    ));
    let mut model = TerminalCompletionModel::new(items, LineContext::new("", 0));
    assert_eq!(
        labels(&mut model),
        [
            ".build", "build", "cglicenses.json", "cgmanifest.json", "cli", "CodeQL.yml", ".configurations", "CONTRIBUTING.md", ".devcontainer", ".editorconfig",
            "eslint.config.js", ".eslint-ignore", ".eslint-plugin-local", "extensions", ".gitattributes", ".git-blame-ignore-revs", ".github", ".gitignore", "gulpfile.js",
            "LICENSE.txt", ".lsifrc.json", ".mailmap", ".mention-bot", "node_modules", ".npmrc", ".nvmrc", "out", "package.json", "package-lock.json", "product.json",
            ".profile-oss", "README.md", "remote", "resources", "scripts", "SECURITY.md", "src", "test", "ThirdPartyNotices.txt", "tsfmt.json", ".vscode", ".vscode-test",
            ".vscode-test.js", "__init__.py", "__pycache",
        ]
    );
}

#[test]
fn punctuation_chars_should_be_below_other_methods() {
    assert_items(["a", "b", ",", ";", ":", "c", "[", "..."].map(method).to_vec(), "", &["a", "b", "c", ",", ";", ":", "[", "..."]);
}

#[test]
fn punctuation_chars_should_be_below_other_files() {
    assert_items(["..", "...", "../", "./a/", "./b/"].map(method).to_vec(), "", &["./a/", "./b/", "..", "...", "../"]);
}

fn inline_items(kind: TerminalCompletionItemKind) -> Vec<TerminalCompletionItem> {
    let mut items = items_of(&["a", "c"], Folder);
    items.extend(items_of(&["b", "d"], File));
    items.push(TerminalCompletionItem::new(TerminalCompletion { kind: Some(kind), replacement_range: Some((0, 0)), ..TerminalCompletion::new("ab", "core") }, None));
    items
}

#[test]
fn inline_suggestion_should_put_on_top_generally() {
    assert_eq!(labels(&mut TerminalCompletionModel::new(inline_items(InlineSuggestion), LineContext::new("", 0)))[0], "ab");
}

#[test]
fn inline_suggestion_should_not_put_on_top_when_there_is_an_exact_match_of_another_item() {
    let labels = labels(&mut TerminalCompletionModel::new(inline_items(InlineSuggestion), LineContext::new("a", 0)));
    assert_ne!(labels[0], "ab");
    assert_eq!(labels[1], "ab");
}

#[test]
fn inline_suggestion_always_on_top_should_put_on_top_generally() {
    assert_eq!(labels(&mut TerminalCompletionModel::new(inline_items(InlineSuggestionAlwaysOnTop), LineContext::new("", 0)))[0], "ab");
}

#[test]
fn inline_suggestion_always_on_top_should_put_on_top_even_if_there_is_an_exact_match_of_another_item() {
    assert_eq!(labels(&mut TerminalCompletionModel::new(inline_items(InlineSuggestionAlwaysOnTop), LineContext::new("a", 0)))[0], "ab");
}

fn arguments(labels: &[&str]) -> Vec<TerminalCompletionItem> {
    items_of(labels, Argument)
}

#[test]
fn git_should_prioritize_main_and_master_branches_for_git_commands() {
    assert_items(arguments(&["feature-branch", "master", "development", "main"]), "git checkout ", &["main", "master", "development", "feature-branch"]);
}

#[test]
fn git_should_prioritize_main_and_master_branches_for_git_switch_command() {
    assert_items(arguments(&["feature-branch", "main", "another-feature", "master"]), "git switch ", &["main", "master", "another-feature", "feature-branch"]);
}

#[test]
fn git_should_not_prioritize_main_and_master_for_non_git_commands() {
    assert_items(arguments(&["feature-branch", "master", "main"]), "ls ", &["feature-branch", "main", "master"]);
}

#[test]
fn git_should_handle_git_commands_with_leading_whitespace() {
    assert_items(arguments(&["feature-branch", "master", "main"]), "  git checkout ", &["main", "master", "feature-branch"]);
}

#[test]
fn git_should_work_with_complex_label_objects() {
    let object = |label: &str, description: &str| {
        let completion = TerminalCompletion {
            kind: Some(Argument),
            replacement_range: Some((0, 1)),
            string_label: false,
            label_description: Some(description.into()),
            ..TerminalCompletion::new(label, "defaultProvider")
        };
        TerminalCompletionItem::new(completion, None)
    };
    let items = vec![object("feature-branch", "Feature branch"), object("master", "Master branch"), object("main", "Main branch")];
    let mut model = TerminalCompletionModel::new(items, LineContext::new("git checkout ", 0));
    let labels: Vec<_> = model.items().iter().map(|item| (item.completion.label.as_str(), item.completion.label_description.as_deref())).collect();
    assert_eq!(labels, [("main", Some("Main branch")), ("master", Some("Master branch")), ("feature-branch", Some("Feature branch"))]);
}

#[test]
fn git_should_not_prioritize_branches_with_similar_names() {
    assert_items(arguments(&["mainline", "masterpiece", "main", "master"]), "git checkout ", &["main", "master", "mainline", "masterpiece"]);
}

#[test]
fn git_should_prioritize_for_git_branch_d() {
    assert_items(arguments(&["main", "master", "dev"]), "git branch -d ", &["main", "master", "dev"]);
}

#[test]
fn mixed_should_sort_arguments_before_flags_and_options() {
    let items = vec![create_item("--verbose", Flag), create_item("--config", Option), create_item("value2", Argument), create_item("value1", Argument), create_item("--all", Flag)];
    assert_items(items, "cmd ", &["value1", "value2", "--all", "--config", "--verbose"]);
}

#[test]
fn mixed_should_sort_by_kind_hierarchy() {
    let items = vec![
        create_item("file.txt", File),
        create_item("--flag", Flag),
        create_item("arg", Argument),
        create_item("method", Method),
        create_item("folder/", Folder),
        create_item("--option", Option),
        create_item("alias", Alias),
        create_item("file2.txt", SymbolicLinkFile),
        create_item("folder2/", SymbolicLinkFolder),
    ];
    assert_items(items, "", &["alias", "method", "arg", "--flag", "--option", "file2.txt", "file.txt", "folder/", "folder2/"]);
}

#[test]
fn typing_more_chars_refilters_the_filtered_items_and_deleting_refilters_all() {
    let mut model = TerminalCompletionModel::new(["src", "scripts", "out"].map(method).to_vec(), LineContext::new("s", 0));
    assert_eq!(labels(&mut model), ["scripts", "src"]);
    assert_eq!(model.label_length_percentile(), 7);
    model.set_line_context(LineContext::new("scr", 2));
    assert_eq!(labels(&mut model), ["scripts"]);
    model.set_line_context(LineContext::new("o", 0));
    assert_eq!(labels(&mut model), ["out"]);
    assert_eq!(model.line_context(), &LineContext::new("o", 0));
    model.force_refilter_all();
    assert_eq!(labels(&mut model), ["out"]);
}
