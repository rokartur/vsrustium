//! bulkEditPreview.ts and bulkEditTree.ts checked states (cases after bulkEditPreview.test.ts).

use serde_json::{Value, json};

use super::*;

fn text(path: &str, text: &str, label: Option<&str>, needs_confirmation: bool) -> Value {
    let mut edit = json!({ "resource": { "scheme": "file", "path": path }, "textEdit": { "range": { "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 1 }, "text": text } });
    if let Some(label) = label {
        edit["metadata"] = json!({ "label": label, "needsConfirmation": needs_confirmation });
    }
    edit
}

fn rename(old: &str, new: &str, label: Option<&str>, needs_confirmation: bool) -> Value {
    let mut edit = json!({ "oldResource": { "scheme": "file", "path": old }, "newResource": { "scheme": "file", "path": new } });
    if let Some(label) = label {
        edit["metadata"] = json!({ "label": label, "needsConfirmation": needs_confirmation });
    }
    edit
}

#[test]
fn edits_group_by_file_and_by_category_with_confirmation_categories_first() {
    let preview = Preview::new(vec![text("/b.ts", "x", Some("uri1"), false), text("/a.ts", "y", Some("uri2"), true), text("/b.ts", "z", None, false)], |_| false);
    assert_eq!(preview.files.iter().map(|file| file.key.as_str()).collect::<Vec<_>>(), ["file:///a.ts", "file:///b.ts"]);
    assert_eq!(preview.files[1].text_edits, [0, 2]);
    assert_eq!(preview.categories.iter().map(|category| category.label.as_str()).collect::<Vec<_>>(), ["uri2", "Other", "uri1"]);
    // An edit that needs confirmation starts unchecked.
    assert_eq!(preview.checked, [true, false, true]);
    assert_eq!(preview.checked_edits().len(), 2);
}

#[test]
fn an_edit_to_a_renamed_file_joins_the_rename_and_an_unchecked_rename_unchecks_its_edits() {
    let preview = Preview::new(vec![rename("/old.ts", "/new.ts", Some("rename"), true), text("/new.ts", "x", None, false)], |_| false);
    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.files[0].kind, RENAME | TEXT_EDIT);
    assert_eq!(preview.checked, [false, false]);
    assert!(!preview.has_checked_changes());
}

#[test]
fn checking_a_text_edit_checks_its_files_file_edits() {
    let mut preview = Preview::new(vec![rename("/old.ts", "/new.ts", Some("rename"), true), text("/new.ts", "x", Some("rename"), true)], |_| false);
    let file = preview.files[0].clone();
    assert!(!preview.file_checked(&file, false));
    preview.set_text_edit_checked(&file, 1, true);
    assert_eq!(preview.checked, [true, true]);
    assert!(preview.file_checked(&file, false));
}

#[test]
fn a_text_file_under_a_category_is_disabled_while_its_rename_elsewhere_is_unchecked() {
    let mut preview = Preview::new(vec![rename("/a.ts", "/b.ts", Some("move"), false), text("/a.ts", "x", Some("fix"), false)], |_| false);
    let fix = preview.categories.iter().position(|category| category.label == "fix").unwrap();
    let text_file = preview.categories[fix].files[0].clone();
    assert!(!preview.file_disabled(&text_file, true));
    let rename_category = preview.categories.iter().position(|category| category.label == "move").unwrap();
    preview.set_category_checked(rename_category, false);
    assert!(preview.file_disabled(&text_file, true));
    assert!(!preview.file_checked(&text_file, true));
}

#[test]
fn ignore_if_exists_skips_an_existing_create() {
    let create = json!({ "newResource": { "scheme": "file", "path": "/x.ts" }, "options": { "ignoreIfExists": true } });
    assert!(Preview::new(vec![create.clone()], |_| true).files.is_empty());
    assert_eq!(Preview::new(vec![create], |_| false).files[0].kind, CREATE);
}

#[test]
fn text_edit_parts_take_23_chars_before_and_the_rest_of_the_line_without_tokens() {
    let lines = vec!["const someLongVariableName = oldValue + 1;".to_string()];
    let (prefix, selecting, suffix) = text_edit_parts(&lines, (1, 30), (1, 38), &[0], &[0]);
    assert_eq!(selecting, "oldValue");
    assert_eq!(prefix, "someLongVariableName = ");
    assert_eq!(suffix, " + 1;");
}

#[test]
fn text_edits_apply_from_the_last_to_the_first() {
    let edits = vec![((1, 5), (1, 8), "bar".to_string()), ((2, 1), (2, 1), "// ".to_string()), ((1, 1), (1, 4), "let".to_string())];
    assert_eq!(apply_text_edits("var foo = 1;\nx();", edits), "let bar = 1;\n// x();");
}

#[test]
fn text_edit_parts_grow_the_prefix_to_a_token_start() {
    let lines = vec!["let a = foo(bar);".to_string()];
    // Tokens: `let`, ` `, `a`, ` `, `=`, ` `, `foo`, `(`, `bar`, `)`, `;`.
    let tokens = [0, 3, 4, 5, 6, 7, 8, 11, 12, 15, 16];
    let (prefix, selecting, suffix) = text_edit_parts(&lines, (1, 13), (1, 16), &tokens, &tokens);
    assert_eq!((prefix.as_str(), selecting.as_str(), suffix.as_str()), ("let a = foo(", "bar", ");"));
}
