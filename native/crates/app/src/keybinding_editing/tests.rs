use super::*;
use crate::keybindings::parse_keybinding;
use crate::keybindings_editor::Source;

fn row(command: &str, keybinding: &str, when: &str, is_default: bool) -> Row {
    Row {
        keybinding: parse_keybinding(keybinding),
        command: command.into(),
        command_label: String::new(),
        args: None,
        when: when.into(),
        is_default,
        source: if is_default { Source::System } else { Source::User },
    }
}

fn options() -> FormattingOptions {
    FormattingOptions { tab_size: 4, insert_spaces: true, eol: "\n".into() }
}

fn file(entries: Value) -> String {
    entries.to_string()
}

fn applied(content: Option<&str>, row: &Row, change: Change) -> Value {
    let written = apply(content, row, change, &options()).unwrap();
    json::parse(&written, &mut Vec::new(), ParseOptions::default()).unwrap()
}

const PARSE_ERRORS: &str = "Unable to write to the keybindings configuration file. Please open it to correct errors/warnings in the file and try again.";

#[test]
fn errors_cases() {
    let some = row("some command", "escape", "", true);
    let edit = || Change::Edit { key: "alt+c", when: None };
    assert_eq!(apply(Some(",,,,,,,,,,,,,,"), &some, edit(), &options()).unwrap_err(), PARSE_ERRORS);
    assert_eq!(apply(Some(r#"[{"key": }]"#), &some, edit(), &options()).unwrap_err(), PARSE_ERRORS);
    assert_eq!(
        apply(Some(r#"{"key": "alt+c", "command": "hello"}"#), &some, edit(), &options()).unwrap_err(),
        "Unable to write to the keybindings configuration file. It has an object which is not of type Array. Please open the file to clean up and try again."
    );
}

#[test]
fn edit_a_default_keybinding() {
    let a = row("a", "escape", "", true);
    let expected = json!([{ "key": "alt+c", "command": "a" }, { "key": "escape", "command": "-a" }]);
    assert_eq!(applied(Some(""), &a, Change::Edit { key: "alt+c", when: None }), expected);
    assert_eq!(applied(None, &a, Change::Edit { key: "alt+c", when: None }), expected);
    assert_eq!(applied(Some("[]"), &a, Change::Edit { key: "alt+c", when: None }), expected);
    let existing = file(json!([{ "command": "b", "key": "shift+c" }]));
    assert_eq!(
        applied(Some(&existing), &a, Change::Edit { key: "alt+c", when: None }),
        json!([{ "key": "shift+c", "command": "b" }, { "key": "alt+c", "command": "a" }, { "key": "escape", "command": "-a" }])
    );
}

#[test]
fn empty_file_gets_the_header() {
    let written = apply(Some(""), &row("a", "", "", true), Change::Add { key: "alt+c", when: None }, &options()).unwrap();
    assert!(written.starts_with("// Place your key bindings in this file to override the defaults\n["), "{written}");
    let written = apply(Some("// only a comment"), &row("a", "", "", true), Change::Add { key: "alt+c", when: None }, &options()).unwrap();
    assert!(written.starts_with("// only a comment\n["), "{written}");
}

#[test]
fn add_keybindings() {
    let expected = json!([{ "key": "alt+c", "command": "a" }]);
    assert_eq!(applied(Some("[]"), &row("a", "escape", "", true), Change::Add { key: "alt+c", when: None }), expected);
    assert_eq!(applied(Some("[]"), &row("a", "", "", true), Change::Add { key: "alt+c", when: None }), expected);
    assert_eq!(applied(Some("[]"), &row("a", "", "", true), Change::Edit { key: "alt+c", when: None }), expected);
}

#[test]
fn add_a_keybinding_preserves_command_arguments() {
    let with_args = Row { args: Some(json!({ "source": "keyboardShortcut" })), ..row("b", "", "", false) };
    assert_eq!(applied(Some("[]"), &with_args, Change::Add { key: "ctrl+cmd+a", when: None }), json!([{ "key": "ctrl+cmd+a", "command": "b", "args": { "source": "keyboardShortcut" } }]));
}

#[test]
fn edit_an_user_keybinding() {
    let b = row("b", "escape", "", false);
    let content = file(json!([{ "key": "escape", "command": "b" }]));
    assert_eq!(applied(Some(&content), &b, Change::Edit { key: "alt+c", when: None }), json!([{ "key": "alt+c", "command": "b" }]));
    let content = file(json!([{ "key": "escape", "command": "b" }, { "key": "alt+shift+g", "command": "c" }]));
    assert_eq!(applied(Some(&content), &b, Change::Edit { key: "alt+c", when: None }), json!([{ "key": "alt+c", "command": "b" }, { "key": "alt+shift+g", "command": "c" }]));
}

#[test]
fn remove_a_default_keybinding_once() {
    let a = row("a", "alt+c", "", true);
    let mut content = "[]".to_string();
    for _ in 0..5 {
        content = apply(Some(&content), &a, Change::Remove, &options()).unwrap();
    }
    assert_eq!(json::parse(&content, &mut Vec::new(), ParseOptions::default()).unwrap(), json!([{ "key": "alt+c", "command": "-a" }]));
}

#[test]
fn remove_and_reset_user_keybindings() {
    let b = row("b", "alt+c", "", false);
    let content = file(json!([{ "key": "alt+c", "command": "b" }]));
    assert_eq!(applied(Some(&content), &b, Change::Remove), json!([]));
    assert_eq!(applied(Some(&content), &b, Change::Reset), json!([]));
    let unbound = row("b", "", "", false);
    let content = file(json!([{ "key": "alt+c", "command": "-b" }]));
    assert_eq!(applied(Some(&content), &unbound, Change::Reset), json!([]));
    let content = file(json!([{ "key": "alt+c", "command": "-b" }, { "key": "alt+shift+c", "command": "-b" }, { "key": "escape", "command": "-b" }]));
    assert_eq!(applied(Some(&content), &unbound, Change::Reset), json!([]));
}

#[test]
fn add_and_update_when_expressions() {
    let a = row("a", "", "", false);
    let removed = file(json!([{ "key": "alt+c", "command": "-a" }]));
    assert_eq!(
        applied(Some(&removed), &a, Change::Edit { key: "shift+alt+c", when: None }),
        json!([{ "key": "alt+c", "command": "-a" }, { "key": "shift+alt+c", "command": "a" }])
    );
    assert_eq!(
        applied(Some(&removed), &a, Change::Edit { key: "shift+alt+c", when: Some("editorTextFocus") }),
        json!([{ "key": "alt+c", "command": "-a" }, { "key": "shift+alt+c", "command": "a", "when": "editorTextFocus" }])
    );

    let removed_when = json!({ "key": "alt+c", "command": "-a", "when": "editorTextFocus && !editorReadonly" });
    let content = file(json!([removed_when]));
    assert_eq!(
        applied(Some(&content), &a, Change::Edit { key: "shift+alt+c", when: Some("editorTextFocus") }),
        json!([removed_when, { "key": "shift+alt+c", "command": "a", "when": "editorTextFocus" }])
    );
    let content = file(json!([removed_when, { "key": "shift+alt+c", "command": "a", "when": "editorTextFocus && !editorReadonly" }]));
    let with_when = row("a", "", "editorTextFocus && !editorReadonly", false);
    assert_eq!(
        applied(Some(&content), &with_when, Change::Edit { key: "shift+alt+c", when: Some("editorTextFocus") }),
        json!([removed_when, { "key": "shift+alt+c", "command": "a", "when": "editorTextFocus" }])
    );
}

#[test]
fn edits_keep_comments() {
    let content = "// mine\n[\n    // a comment\n    { \"key\": \"escape\", \"command\": \"b\" }\n]";
    let written = apply(Some(content), &row("b", "escape", "", false), Change::Edit { key: "alt+c", when: None }, &options()).unwrap();
    assert!(written.contains("// a comment") && written.starts_with("// mine"), "{written}");
}
