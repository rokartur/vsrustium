use super::*;
use crate::keybindings::parse_keybinding;

fn item(command: &str, keybinding: &str, when: Option<&str>, is_default: bool) -> Item {
    Item::new(parse_keybinding(keybinding), command, None, when.and_then(Expr::deserialize), is_default)
}

fn user(command: &str, keybinding: &str) -> Item {
    item(command, keybinding, Some("whenContext1 && whenContext2"), false)
}

fn titled(id: &str, title: &str) -> MenuItem {
    MenuItem { id: id.into(), title: title.into(), category: None, when: None, precondition: None, source: None }
}

/// `registerCommandWithTitle`: each menu item's command is registered too.
fn model(items: &[Item], menu: &[MenuItem], registered: &[&str]) -> Model {
    let registered: Vec<&str> = menu.iter().map(|item| item.id.as_str()).chain(registered.iter().copied()).collect();
    Model::new(items, items, menu, &registered)
}

/// The commands and keybinding labels of the rows that have a keybinding.
fn bound(entries: &[Entry]) -> Vec<(String, String)> {
    entries.iter().filter(|entry| !entry.row.keybinding.is_empty()).map(|entry| (entry.row.command.clone(), keybindings::label(&entry.row.keybinding))).collect()
}

fn pairs(items: &[&Item]) -> Vec<(String, String)> {
    items.iter().map(|item| (item.command.clone(), keybindings::label(&item.keybinding))).collect()
}

fn of<'a>(entries: &'a [Entry], command: &str) -> Vec<&'a Entry> {
    entries.iter().filter(|entry| entry.row.command == command).collect()
}

fn chord(parts: &str) -> ChordMatch {
    let mut matched = ChordMatch::default();
    for part in parts.split(' ').filter(|part| !part.is_empty()) {
        match part {
            "ctrl" => matched.ctrl = true,
            "shift" => matched.shift = true,
            "alt" => matched.alt = true,
            "meta" => matched.meta = true,
            _ => matched.key = true,
        }
    }
    matched
}

fn keybinding_matches_of(entries: &[Entry], command: &str) -> [ChordMatch; 2] {
    let entries = of(entries, command);
    assert_eq!(entries.len(), 1);
    entries[0].matches.keybinding.expect("the keybinding matched")
}

#[test]
fn fetch_returns_default_keybindings() {
    let items = [item("a1", "escape", None, true), item("b1", "escape escape", None, true)];
    assert_eq!(bound(&model(&items, &[], &[]).fetch("", false)), pairs(&[&items[0], &items[1]]));
}

#[test]
fn fetch_returns_distinct_keybindings() {
    let items = [item("a1", "escape", None, true), item("a1", "escape", None, true)];
    assert_eq!(bound(&model(&items, &[], &[]).fetch("", false)), pairs(&[&items[0]]));
}

#[test]
fn fetch_returns_default_keybindings_sorted_by_command_id() {
    let items = [item("b1", "escape", None, true), item("c1", "escape escape", None, true), item("a1", "backspace", None, true)];
    assert_eq!(bound(&model(&items, &[], &[]).fetch("", false)), pairs(&[&items[2], &items[0], &items[1]]));
}

#[test]
fn fetch_returns_user_keybinding_first_if_default_and_user_has_same_id() {
    let items = [item("b1", "escape", None, true), item("b1", "escape escape", None, false)];
    assert_eq!(bound(&model(&items, &[], &[]).fetch("", false)), pairs(&[&items[1], &items[0]]));
}

#[test]
fn fetch_returns_keybinding_with_titles_first() {
    let items = [item("a1", "escape", None, true), item("b1", "escape escape", None, true), item("c1", "escape escape", None, true), item("d1", "escape escape", None, true)];
    let menu = [titled("b1", "B Title"), titled("d1", "A Title")];
    assert_eq!(bound(&model(&items, &menu, &[]).fetch("", false)), pairs(&[&items[3], &items[1], &items[0], &items[2]]));
}

#[test]
fn fetch_returns_keybinding_with_user_first_if_title_and_id_matches() {
    let items = [item("a1", "escape", None, true), item("b1", "escape escape", None, true), item("c1", "escape escape", None, true), item("b1", "escape", None, false)];
    let menu = [titled("b1", "Same Title")];
    assert_eq!(bound(&model(&items, &menu, &[]).fetch("", false)), pairs(&[&items[3], &items[1], &items[0], &items[2]]));
}

#[test]
fn fetch_returns_default_keybindings_sorted_by_precedence() {
    let items = [item("b1", "escape", None, true), item("c1", "escape escape", None, true), item("a1", "backspace", None, true)];
    assert_eq!(bound(&model(&items, &[], &[]).fetch("", true)), pairs(&[&items[0], &items[1], &items[2]]));
}

#[test]
fn convert_keybinding_with_and_without_title_to_entry() {
    let items = [item("a1", "escape", Some("context1 && context2"), true)];
    let entry = &model(&items, &[], &[]).fetch("", false)[0];
    assert_eq!((entry.row.command.as_str(), entry.row.command_label.as_str(), entry.row.when.as_str()), ("a1", "", "context1 && context2"));
    assert_eq!(keybindings::aria_label(&entry.row.keybinding), "Escape");

    let entry = &model(&items, &[titled("a1", "Some Title")], &[]).fetch("", false)[0];
    assert_eq!(entry.row.command_label, "Some Title");
}

#[test]
fn workbench_commands_take_the_reference_label() {
    let items = [item("copyFilePath", "alt+cmd+c", None, true), item("cursorDown", "down", None, true)];
    let entries = model(&items, &[], &[]).fetch("", false);
    let labels: Vec<&str> = entries[..2].iter().map(|entry| entry.row.command_label.as_str()).collect();
    assert_eq!(labels, ["File: Copy Path of Active File", ""]);
}

#[test]
fn convert_unbound_commands_to_entries() {
    let entries = model(&[], &[titled("a1", "some title")], &["command_without_keybinding", "_private", "vscode.open"]).fetch("", false);
    let commands: Vec<(&str, &str, bool, &str)> = entries.iter().map(|entry| (entry.row.command.as_str(), entry.row.command_label.as_str(), entry.row.keybinding.is_empty(), entry.row.when.as_str())).collect();
    let editor_actions = EDITOR_ACTIONS.len();
    assert_eq!(commands.len(), 2 + editor_actions);
    assert!(commands.contains(&("a1", "some title", true, "")));
    assert!(commands.contains(&("command_without_keybinding", "", true, "")));
    assert_eq!(commands.last(), Some(&("command_without_keybinding", "", true, "")), "untitled commands come last");
}

#[test]
fn filter_by_command_id_and_title() {
    let id = "workbench.action.view.size.1";
    let menu = [titled(id, "some title"), titled("a1", "Increase view size")];
    let model = model(&[], &menu, &[]);
    assert_eq!(of(&model.fetch("workbench action view size", false), id).len(), 1);
    assert_eq!(of(&model.fetch("increase size", false), "a1").len(), 1);
}

#[test]
fn filter_by_source() {
    let items = [item("a1", "escape", Some("context1 && context2"), true), item("b1", "escape", Some("context1 && context2"), false)];
    let model = model(&items, &[], &[]);
    assert_eq!(of(&model.fetch("system", false), "a1").len(), 1);
    assert_eq!(of(&model.fetch("user", false), "b1").len(), 1);
    let defaults = model.fetch("@source: default", false);
    assert_eq!((of(&defaults, "a1").len(), of(&defaults, "b1").len()), (1, 0));
    let users = model.fetch("@source: user", false);
    assert_eq!((of(&users, "a1").len(), of(&users, "b1").len()), (0, 1));
}

#[test]
fn filter_by_command_prefix() {
    let items = [item("a1", "escape", Some("context1 && context2"), true), item("b1", "alt+escape", Some("whenContext1 && whenContext2"), true)];
    let actual = model(&items, &[], &[]).fetch("@command:a1", false);
    assert_eq!(actual.iter().map(|entry| entry.row.command.as_str()).collect::<Vec<_>>(), ["a1"]);

    let items = [item("a1", "escape", Some("context1 && context2"), true), item("a1", "alt+escape", Some("whenContext1 && whenContext2"), true)];
    let actual = model(&items, &[], &[]).fetch("@command:a1", false);
    assert_eq!(actual.iter().map(|entry| entry.row.command.as_str()).collect::<Vec<_>>(), ["a1", "a1"]);
}

#[test]
fn filter_by_command_and_when() {
    let items = [item("a1", "escape", Some("context1 && context2"), true), item("a1", "alt+escape", Some("whenContext1"), true)];
    let model = model(&items, &[], &[]);
    let actual = model.fetch("@command:a1 +when:whenContext1", false);
    assert_eq!(actual.iter().map(|entry| keybindings::label(&entry.row.keybinding)).collect::<Vec<_>>(), ["⌥Escape"]);
    let actual = model.fetch("@command:a1 +when:other", false);
    assert_eq!(actual.iter().map(|entry| (entry.row.keybinding.is_empty(), entry.row.when.as_str(), entry.row.is_default)).collect::<Vec<_>>(), [(true, "other", false)]);
}

#[test]
fn filter_by_when_context() {
    let items = [user("a1", "escape")];
    assert_eq!(of(&model(&items, &[], &[]).fetch("when context", false), "a1").len(), 1);
}

#[test]
fn filter_by_modifier_words() {
    for (search, matching, other, expected) in [
        ("cmd", "cmd+escape", "escape", "meta"),
        ("meta", "cmd+escape", "shift+escape", "meta"),
        ("command", "cmd+escape", "alt+escape", "meta"),
        ("alt", "alt+escape", "cmd+escape", "alt"),
        ("option", "alt+escape", "cmd+escape", "alt"),
        ("ctrl", "ctrl+escape", "shift+escape", "ctrl"),
        ("control", "ctrl+escape", "cmd+escape", "ctrl"),
        ("shift", "shift+escape", "cmd+escape", "shift"),
        ("arrow", "shift+right", "cmd+escape", "key"),
        ("alt right", "alt+right", "cmd+right", "alt key"),
        ("alt cmd esc", "alt+cmd+escape", "cmd+escape", "alt meta key"),
        ("cmd shift esc", "shift+cmd+escape", "cmd+escape", "shift meta key"),
        ("cmd shift esc", "shift+cmd+escape delete", "cmd+escape", "shift meta key"),
    ] {
        let items = [user("a1", matching), user("b1", other)];
        let entries = model(&items, &[], &[]).fetch(search, false);
        assert_eq!(keybinding_matches_of(&entries, "a1"), [chord(expected), chord("")], "{search}");
    }
}

#[test]
fn filter_by_key_and_modifier() {
    let items = [user("a1", "alt+right"), user("b1", "cmd+right")];
    assert!(of(&model(&items, &[], &[]).fetch("right alt", false), "a1").is_empty());
}

#[test]
fn filter_matches_in_chord_part() {
    let items = [user("a1", "shift+cmd+escape delete"), user("b1", "cmd+escape")];
    let entries = model(&items, &[], &[]).fetch("cmd del", false);
    assert_eq!(keybinding_matches_of(&entries, "a1"), [chord("meta"), chord("key")]);

    let items = [user("a1", "shift+cmd+escape delete"), user("b1", "shift+cmd+escape up")];
    let entries = model(&items, &[], &[]).fetch("cmd shift esc del", false);
    assert_eq!(keybinding_matches_of(&entries, "a1"), [chord("shift meta key"), chord("key")]);
}

#[test]
fn filter_exact_matches() {
    for (search, matching, other, first, second) in [
        ("\"ctrl c\"", "ctrl+c", "shift+cmd+escape ctrl+c", "ctrl key", ""),
        ("\"shift meta escape ctrl c\"", "shift+cmd+escape ctrl+c", "ctrl+c", "shift meta key", "ctrl key"),
        ("\"control+c\"", "ctrl+c", "shift+cmd+escape ctrl+c", "ctrl key", ""),
        ("@keybinding:control+c", "ctrl+c", "shift+cmd+escape ctrl+c", "ctrl key", ""),
        ("\"shift+meta+escape ctrl+c\"", "shift+cmd+escape ctrl+c", "ctrl+c", "shift meta key", "ctrl key"),
        ("@keybinding:\"shift+meta+escape ctrl+c\"", "shift+cmd+escape ctrl+c", "ctrl+c", "shift meta key", "ctrl key"),
        ("\"control+k\"", "ctrl+k ctrl+c", "shift+cmd+escape ctrl+c", "ctrl key", ""),
    ] {
        let items = [user("a1", matching), user("b1", other)];
        let entries = model(&items, &[], &[]).fetch(search, false);
        assert_eq!(keybinding_matches_of(&entries, "a1"), [chord(first), chord(second)], "{search}");
    }
}

#[test]
fn filter_exact_matches_with_first_and_chord_part_no_results() {
    let items = [user("a1", "shift+cmd+escape cmd+delete"), user("b1", "shift+cmd+escape up")];
    assert!(of(&model(&items, &[], &[]).fetch("\"cmd shift esc del\"", false), "a1").is_empty());
}

#[test]
fn filter_exact_matches_with_space_32993() {
    let items = [user("a1", "ctrl+space"), user("b1", "ctrl+backspace")];
    assert_eq!(of(&model(&items, &[], &[]).fetch("\"ctrl+space\"", false), "a1").len(), 1);
}

#[test]
fn filter_exact_matches_with_user_settings_label() {
    let items = [item("a1", "down", None, true), item("down", "escape", None, true)];
    let entries = model(&items, &[], &[]).fetch("\"down\"", false);
    assert_eq!(keybinding_matches_of(&entries, "a1")[0], chord("key"));
}

#[test]
fn filter_modifiers_are_not_matched_when_not_completely_matched() {
    for term in ["alt.1", "abcaltdef.1"] {
        let command = format!("command.{term}");
        let items = [item(&command, "escape", None, false), item("some_command", "alt+escape", None, false)];
        let entries = model(&items, &[], &[]).fetch(term, false);
        assert_eq!(entries.len(), 1, "{term}");
        assert_eq!(entries[0].row.command, command);
        assert_eq!(entries[0].matches.command_id.as_ref().map(Vec::len), Some(1));
    }
}

#[test]
fn filter_modifiers_are_matched_with_complete_term() {
    let items = [item("command.1", "alt+escape", None, false), item("some_command", "escape", None, false)];
    let entries = model(&items, &[], &[]).fetch("alt", false);
    assert_eq!(keybinding_matches_of(&entries, "command.1")[0], chord("alt"));
}

#[test]
fn filter_by_extension() {
    let foo = ExtensionSource { id: "foo".into(), display_name: Some("foo bar".into()), is_builtin: false };
    let bar = ExtensionSource { id: "bar".into(), display_name: Some("bar foo".into()), is_builtin: false };
    let mut contributed = item("command.1", "alt+escape", None, true);
    contributed.extension = Some(foo);
    let items = [contributed, item("command.2", "", None, true)];
    let menu = [MenuItem { category: Some("category".into()), source: Some(bar), ..titled("command.2", "title") }];
    let model = model(&items, &menu, &[]);
    let commands = |search| model.fetch(search, false).into_iter().map(|entry| entry.row.command).collect::<Vec<_>>();
    assert_eq!(commands("@ext:foo"), ["command.1"]);
    assert_eq!(commands("@ext:\"bar foo\""), ["command.2"]);
    assert_eq!(commands("@source: extension"), ["command.1", "command.2"]);
}
