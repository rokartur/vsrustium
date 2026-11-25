use serde_json::json;

use super::*;

#[test]
fn from_dto_reads_an_object_label_markdown_docs_and_utf16_ranges() {
    let dto = json!({
        "label": { "label": "main", "description": "Main branch", "detail": " (HEAD)" },
        "detail": "branch",
        "documentation": { "value": "**docs**" },
        "kind": 11,
        "isKeyword": true,
        "replacementRange": [6, 8],
    });
    // The emoji is two UTF-16 units and one char.
    let completion = TerminalCompletion::from_dto(&dto, "ext:git", "git 😀 ma").expect("a label");
    assert_eq!(completion.label, "main");
    assert!(!completion.string_label);
    assert_eq!(completion.label_description.as_deref(), Some("Main branch"));
    assert_eq!(completion.label_detail.as_deref(), Some(" (HEAD)"));
    assert_eq!(completion.detail.as_deref(), Some("branch"));
    assert_eq!(completion.documentation.as_deref(), Some("**docs**"));
    assert_eq!(completion.kind, Some(TerminalCompletionItemKind::Branch));
    assert_eq!(completion.provider, "ext:git");
    assert!(completion.is_keyword);
    assert_eq!(completion.replacement_range, Some((5, 7)));
}

#[test]
fn from_dto_reads_a_string_label_and_rejects_a_missing_one() {
    let completion = TerminalCompletion::from_dto(&json!({ "label": "ls", "documentation": "list", "kind": 101, "replacementRange": [0, 2] }), "p", "ls").expect("a label");
    assert!(completion.string_label);
    assert_eq!(completion.documentation.as_deref(), Some("list"));
    assert_eq!(completion.kind, Some(TerminalCompletionItemKind::InlineSuggestionAlwaysOnTop));
    assert_eq!(completion.replacement_range, Some((0, 2)));
    assert_eq!(TerminalCompletion::from_dto(&json!({ "replacementRange": [0, 0] }), "p", ""), None);
}

fn item(label: &str, kind: TerminalCompletionItemKind) -> TerminalCompletionItem {
    TerminalCompletionItem::new(TerminalCompletion { kind: Some(kind), ..TerminalCompletion::new(label, "p") }, None)
}

#[test]
fn files_split_their_extension_except_dotfiles() {
    let file = item("Package.JSON", TerminalCompletionItemKind::File);
    assert_eq!((file.label_low.as_str(), file.label_low_exclude_file_ext.as_str(), file.file_ext_low.as_str()), ("package.json", "package", "json"));
    let dotfile = item(".gitignore", TerminalCompletionItemKind::File);
    assert_eq!((dotfile.label_low_exclude_file_ext.as_str(), dotfile.file_ext_low.as_str()), (".gitignore", ""));
    let method = item("a.b", TerminalCompletionItemKind::Method);
    assert_eq!((method.label_low_exclude_file_ext.as_str(), method.file_ext_low.as_str()), ("a.b", ""));
}

#[test]
fn folders_normalize_windows_separators_and_drop_the_trailing_one() {
    assert_eq!(item("./src/", TerminalCompletionItemKind::Folder).label_low_normalized_path, "./src");
    assert_eq!(item(".\\Src\\", TerminalCompletionItemKind::Folder).label_low_normalized_path, "./src");
    let branch = TerminalCompletionItem::new(TerminalCompletion { kind: Some(TerminalCompletionItemKind::Branch), ..TerminalCompletion::new("origin/main", "p") }, Some('\\'));
    assert_eq!(branch.label_low, "origin\\main");
}

#[test]
fn punctuation_and_underscore_labels_are_penalized() {
    assert_eq!(item("__init__.py", TerminalCompletionItemKind::File).punctuation_penalty, 1);
    assert_eq!(item("./_private/", TerminalCompletionItemKind::Folder).punctuation_penalty, 1);
    assert_eq!(item("...", TerminalCompletionItemKind::Method).punctuation_penalty, 1);
    assert_eq!(item("./a/", TerminalCompletionItemKind::Method).punctuation_penalty, 0);
    assert_eq!(item("a_b", TerminalCompletionItemKind::Method).punctuation_penalty, 0);
}
