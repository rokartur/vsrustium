//! Ported from platform/keybinding/test/common/keybindingResolver.test.ts and keybindingLabels.test.ts, and
//! base/test/common/keybindings.test.ts. Keybindings are written as user settings strings instead of
//! `KeyMod | KeyCode` numbers; the macOS cases only, as the port runs on macOS.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Item, Resolution, Resolver, aria_label, label, parse_keybinding, user_settings_label};
use crate::contextkey::Expr;

fn context(values: &[(&str, Value)]) -> HashMap<String, Value> {
    values.iter().map(|(key, value)| (key.to_string(), value.clone())).collect()
}

fn kb_item(keybinding: &str, command: &str, args: Option<Value>, when: Option<Expr>, is_default: bool) -> Item {
    Item::new(parse_keybinding(keybinding), command, args, when, is_default)
}

fn equals(key: &str, value: &str) -> Option<Expr> {
    Some(Expr::key_equals(key, json!(value)))
}

/// What `assert.deepStrictEqual` compares on the items of a removal test.
fn summary(items: &[Item]) -> Vec<(String, String, Option<String>, bool)> {
    items.iter().map(|item| (user_settings_label(&item.keybinding), item.command.clone(), item.when.as_ref().map(Expr::serialize), item.is_default)).collect()
}

fn chord(key: &str) -> super::Chord {
    parse_keybinding(key)[0]
}

#[test]
fn resolve_key() {
    let when = Expr::key_equals("bar", json!("baz"));
    let item = kb_item("cmd+shift+z", "yes", None, Some(when.clone()), true);
    assert!(when.evaluate(&context(&[("bar", json!("baz"))])));
    assert!(!when.evaluate(&context(&[("bar", json!("bz"))])));
    let resolver = Resolver::new(vec![item], vec![]);
    let found = |bar: &str| resolver.resolve(&context(&[("bar", json!(bar))]), &[], chord("cmd+shift+z"));
    assert_eq!(found("baz"), Resolution::KbFound { command: "yes".into(), args: None, bubble: false });
    assert_eq!(found("bz"), Resolution::NoMatchingKb);
}

#[test]
fn resolve_key_with_arguments() {
    let args = json!({ "text": "no" });
    let item = kb_item("cmd+shift+z", "yes", Some(args.clone()), equals("bar", "baz"), true);
    let resolver = Resolver::new(vec![item], vec![]);
    let result = resolver.resolve(&context(&[("bar", json!("baz"))]), &[], chord("cmd+shift+z"));
    assert_eq!(result, Resolution::KbFound { command: "yes".into(), args: Some(args), bubble: false });
}

fn assert_removals(rules: Vec<Item>, expected: Vec<Item>) {
    assert_eq!(summary(&Resolver::handle_removals(rules)), summary(&expected));
}

#[test]
fn removals_simple() {
    let defaults = || vec![kb_item("a", "yes1", None, equals("1", "a"), true), kb_item("b", "yes2", None, equals("2", "b"), true)];
    assert_removals(vec![kb_item("a", "yes1", None, equals("1", "a"), true), kb_item("b", "yes2", None, equals("2", "b"), false)], vec![
        kb_item("a", "yes1", None, equals("1", "a"), true),
        kb_item("b", "yes2", None, equals("2", "b"), false),
    ]);
    let mut rules = defaults();
    rules.push(kb_item("c", "yes3", None, equals("3", "c"), false));
    let mut expected = defaults();
    expected.push(kb_item("c", "yes3", None, equals("3", "c"), false));
    assert_removals(rules, expected);
}

#[test]
fn removals_that_do_not_match() {
    let defaults = || vec![kb_item("a", "yes1", None, equals("1", "a"), true), kb_item("b", "yes2", None, equals("2", "b"), true)];
    // not matching when
    let mut rules = defaults();
    rules.push(kb_item("a", "-yes1", None, equals("1", "b"), false));
    assert_removals(rules, defaults());
    // not matching keybinding
    let mut rules = defaults();
    rules.push(kb_item("b", "-yes1", None, equals("1", "a"), false));
    assert_removals(rules, defaults());
}

#[test]
fn removals_that_match() {
    let defaults = || vec![kb_item("a", "yes1", None, equals("1", "a"), true), kb_item("b", "yes2", None, equals("2", "b"), true)];
    // matching keybinding and when, unspecified keybinding, unspecified when, both unspecified
    for (key, when) in [("a", equals("1", "a")), ("", equals("1", "a")), ("a", None), ("", None)] {
        let mut rules = defaults();
        rules.push(kb_item(key, "-yes1", None, when, false));
        assert_removals(rules, vec![kb_item("b", "yes2", None, equals("2", "b"), true)]);
    }
}

#[test]
fn issue_138997_removal_in_default_list() {
    let rules = vec![kb_item("a", "yes1", None, None, true), kb_item("b", "yes2", None, None, true), kb_item("", "-yes1", None, None, false)];
    assert_removals(rules, vec![kb_item("b", "yes2", None, None, true)]);
}

#[test]
fn issue_612_cannot_remove_keybindings_for_commands_with_caret() {
    let rules = vec![kb_item("a", "^yes1", None, equals("1", "a"), true), kb_item("b", "yes2", None, equals("2", "b"), true), kb_item("a", "-yes1", None, None, false)];
    assert_removals(rules, vec![kb_item("b", "yes2", None, equals("2", "b"), true)]);
}

#[test]
fn issue_140884_unable_to_reassign_f1() {
    let rules = vec![kb_item("a", "command1", None, None, true), kb_item("a", "-command1", None, None, false), kb_item("a", "command1", None, None, false)];
    assert_removals(rules, vec![kb_item("a", "command1", None, None, false)]);
}

#[test]
fn issue_141638_change_when_expression_might_remove_keybinding() {
    let rules = vec![kb_item("a", "command1", None, None, true), kb_item("a", "command1", None, equals("a", "1"), false), kb_item("a", "-command1", None, None, false)];
    assert_removals(rules, vec![kb_item("a", "command1", None, equals("a", "1"), false)]);
}

#[test]
fn issue_157751_auto_quoting_of_context_keys() {
    let default_when = Expr::deserialize("editorTextFocus && activeEditor != workbench.editor.notebook && editorLangId in julia.supportedLanguageIds");
    let removal_when = Expr::deserialize("editorTextFocus && activeEditor != 'workbench.editor.notebook' && editorLangId in 'julia.supportedLanguageIds'");
    assert_removals(vec![kb_item("a", "command1", None, default_when, true), kb_item("a", "-command1", None, removal_when, false)], vec![]);
}

#[test]
fn issue_293802_removal_matches_a_more_specific_default_when() {
    let default_when = Expr::and(vec![Some(Expr::has("inChatInput")), Some(Expr::not("withinEditSessionDiff"))]);
    assert_removals(vec![kb_item("a", "command1", None, default_when, true), kb_item("a", "-command1", None, Some(Expr::has("inChatInput")), false)], vec![]);
}

#[test]
fn removal_with_more_specific_when_does_not_match_broader_default() {
    let removal_when = Expr::and(vec![Some(Expr::has("inChatInput")), Some(Expr::not("withinEditSessionDiff"))]);
    let rules = vec![kb_item("a", "command1", None, Some(Expr::has("inChatInput")), true), kb_item("a", "-command1", None, removal_when, false)];
    assert_removals(rules, vec![kb_item("a", "command1", None, Some(Expr::has("inChatInput")), true)]);
}

#[test]
fn issue_338834_removing_the_chat_steer_or_queue_enter_keybinding() {
    let editing_queue = Expr::key_equals("chatEditingSentRequest", json!("q"));
    let editing_steer = Expr::key_equals("chatEditingSentRequest", json!("st"));
    let editing_queue_or_steer = Expr::or(vec![Some(editing_queue.clone()), Some(editing_steer.clone())]).unwrap();
    let queuing_actions_present = Expr::and(vec![
        Some(Expr::not("chatTranscriptProgressActive")),
        Expr::or(vec![Some(Expr::has("chatSessionRequestInProgress")), Some(editing_queue_or_steer.clone())]),
        Some(Expr::key_not_equals("chatEditingSentRequest", json!("s"))),
    ]);
    let steer_is_default = Expr::key_equals("config.chat.requestQueuing.defaultAction", json!("steer"));
    let default_is_queue = Expr::or(vec![Expr::and(vec![Some(steer_is_default.negate()), Some(editing_queue_or_steer.negate())]), Some(editing_queue)]);
    let default_is_steer = Expr::or(vec![Expr::and(vec![Some(steer_is_default), Some(editing_queue_or_steer.negate())]), Some(editing_steer)]);
    let precondition = Expr::and(vec![Some(Expr::has("chatInputHasText")), Some(Expr::not("chatTranscriptProgressActive")), Some(Expr::not("chatInputBlocked"))]);
    let defaults = [("workbench.action.chat.steerWithMessage", default_is_steer), ("workbench.action.chat.queueMessage", default_is_queue)].map(|(command, effective)| {
        let when = Expr::and(vec![precondition.clone(), Expr::and(vec![Some(Expr::has("inChatInput")), queuing_actions_present.clone(), effective])]);
        kb_item("enter", command, None, when, true)
    });
    for default in defaults {
        let removal = kb_item("enter", &format!("-{}", default.command), None, Expr::deserialize(&default.when.as_ref().unwrap().serialize()), false);
        assert_removals(vec![default, removal], vec![]);
    }
}

#[test]
fn issue_160604_remove_keybindings_with_when_clause() {
    assert_removals(vec![kb_item("a", "command1", None, None, true), kb_item("a", "-command1", None, Some(Expr::True), false)], vec![]);
}

#[test]
fn context_is_entirely_included() {
    let expr = |source: Option<&str>| source.and_then(Expr::deserialize);
    let included = |a: Option<&str>, b: Option<&str>| Resolver::when_is_entirely_included(expr(a).as_ref(), expr(b).as_ref());
    for (a, b) in [
        (None, None),
        (None, Some("true")),
        (Some("true"), None),
        (Some("true"), Some("true")),
        (Some("key1"), None),
        (Some("key1"), Some("")),
        (Some("key1"), Some("key1")),
        (Some("key1"), Some("true")),
        (Some("!key1"), Some("")),
        (Some("!key1"), Some("!key1")),
        (Some("key2"), Some("")),
        (Some("key2"), Some("key2")),
        (Some("key1 && key1 && key2 && key2"), Some("key2")),
        (Some("key1 && key2"), Some("key2")),
        (Some("key1 && key2"), Some("key1")),
        (Some("key1 && key2"), Some("")),
        (Some("key1"), Some("key1 || key2")),
        (Some("key1 || !key1"), Some("key2 || !key2")),
        (Some("key1"), Some("key1 || key2 && key3")),
    ] {
        assert!(included(a, b), "{a:?} includes {b:?}");
    }
    for (a, b) in [
        (Some("key1"), Some("!key1")),
        (Some("!key1"), Some("key1")),
        (Some("key1 && key2"), Some("key3")),
        (Some("key1 && key2"), Some("key4")),
        (Some("key1"), Some("key2")),
        (Some("key1 || key2"), Some("key2")),
        (Some(""), Some("key2")),
        (None, Some("key2")),
    ] {
        assert!(!included(a, b), "{a:?} does not include {b:?}");
    }
}

/// The items of the `resolve command` suite.
fn resolve_command_resolver() -> Resolver {
    let item = |key: &str, command: &str, when: Option<Expr>| kb_item(key, command, None, when, true);
    Resolver::new(
        vec![
            // never matches: its `when` is always overwritten by `second`
            item("x", "first", Expr::and(vec![Some(Expr::key_equals("key1", json!(true))), Some(Expr::key_not_equals("key2", json!(false)))])),
            item("x", "second", Some(Expr::key_equals("key2", json!(true)))),
            item("z", "second", None),
            item("x", "third", Some(Expr::key_equals("key3", json!(true)))),
            item("cmd+y", "fourth", Some(Expr::key_equals("key4", json!(true)))),
            item("cmd+y z", "fifth", None),
            item("", "sixth", None),
            item("cmd+k cmd+u", "seventh", None),
            item("cmd+k cmd+k", "seventh", None),
            item("cmd+k cmd+u", "uncomment lines", None),
            item("cmd+k cmd+c", "comment lines", None),
            item("cmd+g cmd+c", "unreachablechord", None),
            item("cmd+g", "eleven", None),
            item("cmd+k a b", "long multi chord", None),
            item("cmd+b cmd+c", "shadowed by long-multi-chord-2", None),
            item("cmd+b cmd+c i", "long-multi-chord-2", None),
        ],
        vec![],
    )
}

fn assert_lookup(resolver: &Resolver, command: &str, expected: &[&str]) {
    let found: Vec<String> = resolver.lookup_keybindings(command).iter().map(|item| user_settings_label(&item.keybinding)).collect();
    assert_eq!(found, expected, "lookup of {command}");
}

fn assert_resolve(resolver: &Resolver, context: &HashMap<String, Value>, keybinding: &str, command: &str) {
    let chords = parse_keybinding(keybinding);
    for (i, &chord) in chords.iter().enumerate() {
        let result = resolver.resolve(context, &chords[..i], chord);
        if i == chords.len() - 1 {
            assert_eq!(result, Resolution::KbFound { command: command.into(), args: None, bubble: false }, "{command} at chord {i}");
        } else {
            assert_eq!(result, Resolution::MoreChordsNeeded, "{command} enters or continues multi chord at chord {i}");
        }
    }
}

#[test]
fn resolve_command() {
    let resolver = resolve_command_resolver();
    let empty = context(&[]);
    assert_lookup(&resolver, "first", &[]);
    assert_lookup(&resolver, "second", &["z", "x"]);
    assert_resolve(&resolver, &context(&[("key2", json!(true))]), "x", "second");
    assert_resolve(&resolver, &empty, "z", "second");
    assert_lookup(&resolver, "third", &["x"]);
    assert_resolve(&resolver, &context(&[("key3", json!(true))]), "x", "third");
    assert_lookup(&resolver, "fourth", &[]);
    assert_lookup(&resolver, "fifth", &["cmd+y z"]);
    assert_resolve(&resolver, &empty, "cmd+y z", "fifth");
    assert_lookup(&resolver, "seventh", &["cmd+k cmd+k"]);
    assert_resolve(&resolver, &empty, "cmd+k cmd+k", "seventh");
    assert_lookup(&resolver, "uncomment lines", &["cmd+k cmd+u"]);
    assert_resolve(&resolver, &empty, "cmd+k cmd+u", "uncomment lines");
    assert_lookup(&resolver, "comment lines", &["cmd+k cmd+c"]);
    assert_resolve(&resolver, &empty, "cmd+k cmd+c", "comment lines");
    assert_lookup(&resolver, "unreachablechord", &[]);
    assert_lookup(&resolver, "eleven", &["cmd+g"]);
    assert_resolve(&resolver, &empty, "cmd+g", "eleven");
    assert_lookup(&resolver, "sixth", &[]);
    assert_lookup(&resolver, "long multi chord", &["cmd+k a b"]);
    assert_resolve(&resolver, &empty, "cmd+k a b", "long multi chord");
}

#[test]
fn kbs_having_common_prefix_the_one_defined_later_is_returned() {
    assert_resolve(&resolve_command_resolver(), &context(&[]), "cmd+b cmd+c i", "long-multi-chord-2");
}

#[test]
fn mac_us_label() {
    for (keybinding, expected) in [
        ("a", "A"),
        ("cmd+a", "⌘A"),
        ("shift+a", "⇧A"),
        ("alt+a", "⌥A"),
        ("ctrl+a", "⌃A"),
        ("cmd+shift+a", "⇧⌘A"),
        ("cmd+alt+a", "⌥⌘A"),
        ("cmd+ctrl+a", "⌃⌘A"),
        ("shift+alt+a", "⇧⌥A"),
        ("shift+ctrl+a", "⌃⇧A"),
        ("alt+ctrl+a", "⌃⌥A"),
        ("cmd+shift+alt+a", "⇧⌥⌘A"),
        ("cmd+shift+ctrl+a", "⌃⇧⌘A"),
        ("cmd+alt+ctrl+a", "⌃⌥⌘A"),
        ("shift+alt+ctrl+a", "⌃⇧⌥A"),
        ("cmd+shift+alt+ctrl+a", "⌃⇧⌥⌘A"),
        ("cmd+a cmd+b", "⌘A ⌘B"),
        ("left", "←"),
        ("up", "↑"),
        ("right", "→"),
        ("down", "↓"),
    ] {
        assert_eq!(label(&parse_keybinding(keybinding)), expected, "{keybinding}");
    }
}

#[test]
fn mac_aria_and_user_settings_labels() {
    let all = parse_keybinding("cmd+shift+alt+ctrl+a");
    assert_eq!(aria_label(&all), "Control+Shift+Option+Command+A");
    assert_eq!(user_settings_label(&all), "ctrl+shift+alt+cmd+a");
    assert_eq!(user_settings_label(&parse_keybinding("cmd+a cmd+b")), "cmd+a cmd+b");
}

#[test]
fn issue_91235_do_not_end_with_a_plus() {
    assert_eq!(aria_label(&parse_keybinding("cmd+alt+alt")), "Option+Command");
}

#[test]
fn issue_173325_equal_scan_code_is_not_v() {
    assert_ne!(parse_keybinding("ctrl+v"), parse_keybinding("ctrl+[Equal]"));
    assert_eq!(parse_keybinding("ctrl+[Equal]"), parse_keybinding("ctrl+="));
}

#[test]
fn default_table_binds_the_editor_and_palette_keys() {
    let resolver = Resolver::new(super::default_keybindings(&[]), vec![]);
    let editor = context(&[("textInputFocus", json!(true)), ("editorTextFocus", json!(true))]);
    let command = |context: &HashMap<String, Value>, key: &str| match resolver.resolve(context, &[], chord(key)) {
        Resolution::KbFound { command, .. } => command,
        other => format!("{other:?}"),
    };
    assert_eq!(command(&editor, "alt+right"), "cursorWordEndRight");
    assert_eq!(command(&editor, "cmd+c"), "editor.action.clipboardCopyAction");
    assert_eq!(command(&context(&[]), "shift+cmd+p"), "workbench.action.showCommands");
    assert_eq!(command(&context(&[("inQuickOpen", json!(true)), ("inFilesPicker", json!(true))]), "cmd+p"), "workbench.action.quickOpenNavigateNextInFilePicker");
    assert_eq!(resolver.resolve(&context(&[]), &[], chord("cmd+k")), Resolution::MoreChordsNeeded);
}
