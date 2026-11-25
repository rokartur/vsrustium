use std::collections::HashMap;

use serde_json::{Value, json};

use super::*;

fn command(id: &str, title: &str, group: &str, order: f64) -> MenuItem {
    let entry = Entry::Command { id: id.into(), precondition: None, toggled: None, alt: None };
    MenuItem { menu: MenuId::Test, entry, title: title.into(), group: group.into(), order, when: None }
}

fn ids(actions: &[MenuAction]) -> Vec<&str> {
    actions
        .iter()
        .map(|action| match action {
            MenuAction::Command { id, .. } => id.as_str(),
            MenuAction::Submenu { title, .. } => title.as_str(),
            MenuAction::Separator => "-",
        })
        .collect()
}

fn no_context() -> HashMap<String, Value> {
    HashMap::new()
}

#[test]
fn group_sorting() {
    let registry = [command("one", "FOO", "0_hello", 0.), command("two", "FOO", "hello", 0.), command("three", "FOO", "Hello", 0.), command("four", "FOO", "", 0.), command("five", "FOO", "navigation", 0.)];
    assert_eq!(ids(&menu_actions(&registry, MenuId::Test, &no_context())), ["five", "-", "one", "-", "two", "-", "three", "-", "four"]);
}

#[test]
fn in_group_sorting_by_title() {
    let registry = [command("a", "aaa", "Hello", 0.), command("b", "fff", "Hello", 0.), command("c", "zzz", "Hello", 0.)];
    assert_eq!(ids(&menu_actions(&registry, MenuId::Test, &no_context())), ["a", "b", "c"]);
}

#[test]
fn in_group_sorting_by_title_and_order() {
    let registry = [command("a", "aaa", "Hello", 10.), command("b", "fff", "Hello", 0.), command("c", "zzz", "Hello", -1.), command("d", "yyy", "Hello", -1.)];
    assert_eq!(ids(&menu_actions(&registry, MenuId::Test, &no_context())), ["d", "c", "b", "a"]);
}

#[test]
fn in_group_sorting_special_navigation() {
    let registry = [command("a", "aaa", "navigation", 1.3), command("b", "fff", "navigation", 1.2), command("c", "zzz", "navigation", 1.1)];
    assert_eq!(ids(&menu_actions(&registry, MenuId::Test, &no_context())), ["c", "b", "a"]);
}

#[test]
fn when_hides_precondition_disables_and_toggled_checks() {
    let mut hidden = command("hidden", "Hidden", "1", 0.);
    hidden.when = Expr::deserialize("shown");
    let mut disabled = command("disabled", "Disabled", "1", 1.);
    disabled.entry = Entry::Command { id: "disabled".into(), precondition: Expr::deserialize("enabled"), toggled: Expr::deserialize("!enabled"), alt: None };
    let context = HashMap::from([("shown".to_string(), json!(false)), ("enabled".to_string(), json!(false))]);
    let actions = menu_actions(&[hidden, disabled], MenuId::Test, &context);
    assert_eq!(actions, [MenuAction::Command { id: "disabled".into(), title: "Disabled".into(), enabled: false, checked: true, alt: None }]);
}

#[test]
fn submenus_without_actions_are_left_out() {
    let submenu = |title: &str, group: &str| MenuItem { menu: MenuId::Test, entry: Entry::Submenu(MenuId::ThemesSubMenu), title: title.into(), group: group.into(), order: 0., when: None };
    let mut theme = command("theme", "Color Theme", "", 1.);
    theme.menu = MenuId::ThemesSubMenu;
    let empty = MenuItem { entry: Entry::Submenu(MenuId::MenubarHelpMenu), ..submenu("Empty", "2") };
    let actions = menu_actions(&[submenu("Themes", "1"), empty, theme], MenuId::Test, &no_context());
    assert_eq!(actions, [MenuAction::Submenu { title: "Themes".into(), actions: vec![MenuAction::Command { id: "theme".into(), title: "Color Theme".into(), enabled: true, checked: false, alt: None }] }]);
}

fn extension(id: &str, contributes: Value) -> ScannedExtension {
    ScannedExtension { id: id.into(), location: Default::default(), description: json!({ "name": id, "publisher": "test", "contributes": contributes }) }
}

/// menusExtensionPoint.ts: command items take the title and enablement of `contributes.commands`, `group@order`
/// splits, items of unknown commands and unknown menus are skipped.
#[test]
fn extension_menu_items() {
    let extensions = [extension(
        "a",
        json!({
            "commands": [{ "command": "a.open", "title": "Open Preview", "enablement": "ready" }, { "command": "a.alt", "title": "Open to the Side" }],
            "menus": {
                "explorer/context": [
                    { "command": "a.open", "alt": "a.alt", "group": "navigation@2", "when": "resourceLangId == markdown" },
                    { "command": "a.missing", "group": "navigation" },
                    { "command": "a.alt", "group": "4_search@x" }
                ],
                "notAMenu": [{ "command": "a.open" }]
            }
        }),
    )];
    let items = extension_menus(&extensions);
    let summary: Vec<_> = items.iter().map(|item| (item.menu, item.title.as_str(), item.group.as_str(), item.order)).collect();
    assert_eq!(summary, [(MenuId::ExplorerContext, "Open Preview", "navigation", 2.), (MenuId::ExplorerContext, "Open to the Side", "4_search", 0.)]);
    let context = HashMap::from([("resourceLangId".to_string(), json!("markdown"))]);
    let actions = menu_actions(&items, MenuId::ExplorerContext, &context);
    let open = MenuAction::Command { id: "a.open".into(), title: "Open Preview".into(), enabled: false, checked: false, alt: Some(("a.alt".into(), "Open to the Side".into())) };
    assert_eq!(actions[0], open);
    assert_eq!(ids(&menu_actions(&items, MenuId::ExplorerContext, &no_context())), ["a.alt"]);
}

/// `isValidItems`: one invalid item drops the whole menu of that extension.
#[test]
fn an_invalid_item_drops_its_menu() {
    let extensions = [extension(
        "a",
        json!({
            "commands": [{ "command": "a.run", "title": "Run" }],
            "menus": { "editor/context": [{ "command": "a.run" }, { "command": "a.run", "when": 1 }], "editor/title/context": [{ "command": "a.run" }] }
        }),
    )];
    let menus: Vec<_> = extension_menus(&extensions).iter().map(|item| item.menu).collect();
    assert_eq!(menus, [MenuId::EditorTitleContext]);
}

/// `contributes.submenus`: a submenu item opens the submenu's own items; a submenu is added once per menu, the
/// first registration of an id wins, and one without a label is skipped.
#[test]
fn extension_submenus() {
    let extensions = [
        extension(
            "a",
            json!({
                "commands": [{ "command": "a.one", "title": "One" }],
                "submenus": [{ "id": "a.more", "label": "More" }, { "id": "a.blank", "label": "" }],
                "menus": {
                    "editor/context": [{ "submenu": "a.more", "group": "1_more" }, { "submenu": "a.more" }, { "submenu": "a.blank" }],
                    "a.more": [{ "command": "a.one" }]
                }
            }),
        ),
        extension("b", json!({ "submenus": [{ "id": "a.more", "label": "Again" }] })),
    ];
    let items = extension_menus(&extensions);
    let actions = menu_actions(&items, MenuId::EditorContext, &no_context());
    let one = MenuAction::Command { id: "a.one".into(), title: "One".into(), enabled: true, checked: false, alt: None };
    assert_eq!(actions, [MenuAction::Submenu { title: "More".into(), actions: vec![one] }]);
}

#[test]
fn mnemonic_menu_label_drops_mnemonics() {
    assert_eq!(mnemonic_menu_label("Save A&&ll"), "Save All");
    assert_eq!(mnemonic_menu_label("Open (&&O)"), "Open ");
    assert_eq!(mnemonic_menu_label("Fish & Chips"), "Fish & Chips");
}
