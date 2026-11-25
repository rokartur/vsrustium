use super::*;

#[test]
fn next_word_takes_the_whitespace_and_one_word() {
    assert_eq!(next_word("  foo bar"), "  foo");
    assert_eq!(next_word("(x)"), "(");
    assert_eq!(next_word("bar"), "bar");
}

#[test]
fn a_completion_without_range_replaces_the_word_before_the_cursor() {
    let lines = vec!["let co".to_owned()];
    let indentation = crate::editor::Indentation { tab_size: 4, indent_size: None, insert_spaces: true };
    let item = completion_item(&lines, Position::new(0, 6), indentation, 1, 2, &json!({ "insertText": "console", "idx": 3 })).unwrap();
    assert_eq!((item.edit.start, item.edit.end, item.idx), (Position::new(0, 4), Position::new(0, 6), 3));
    let ranged = completion_item(&lines, Position::new(0, 6), indentation, 1, 2, &json!({ "insertText": { "snippet": "co${1:nsole}" }, "range": { "startLineNumber": 1, "startColumn": 5, "endLineNumber": 1, "endColumn": 7 } })).unwrap();
    assert_eq!((ranged.edit.text.as_str(), ranged.snippet.as_deref()), ("console", Some("co${1:nsole}")));
}

/// The `providerId` schema lists each provider as `extensionId:groupId` and `extensionId:*`, once.
#[gpui::test]
fn the_provider_id_schema_lists_each_provider_matcher(cx: &mut gpui::TestAppContext) {
    let (workbench, cx, _, _) = crate::workbench::goto_symbol::tests::open(cx, "inline-provider-ids", "one\n");
    let matchers = workbench.update(cx, |workbench, cx| {
        let provider = |handle: u64, group: Value| vec![json!(handle), json!("*"), json!(false), json!("pub.ext"), json!("1.0.0"), group];
        workbench.language_features.register(REGISTER, provider(1, json!("ghost")));
        workbench.language_features.register(REGISTER, provider(2, Value::Null));
        workbench.language_providers_changed(REGISTER, cx);
        workbench.settings.lock().unwrap().registry.inline_completion_providers.clone()
    });
    assert_eq!(matchers, ["pub.ext:ghost", "pub.ext:*", "pub.ext"]);
}
