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
    disabled.entry = Entry::Command { id: "disabled".into(), precondition: Expr::deserialize("enabled"), toggled: Toggled::when(Expr::deserialize("!enabled")), alt: None };
    let context = HashMap::from([("shown".to_string(), json!(false)), ("enabled".to_string(), json!(false))]);
    let actions = menu_actions(&[hidden, disabled], MenuId::Test, &context);
    assert_eq!(actions, [MenuAction::Command { id: "disabled".into(), title: "Disabled".into(), enabled: false, checked: true, alt: None }]);
}

/// menuService.test.ts 'one-shot and persistent menus use the same group and item ordering': the localized title
/// sorts by its English original, which is the port's only title.
#[test]
fn group_and_item_ordering_with_ties() {
    let mut hidden = command("hidden", "Hidden", "", 0.);
    hidden.when = Some(Expr::has("hidden"));
    let registry = [
        command("z", "Z", "", 0.),
        hidden,
        command("a", "A", "", 0.),
        command("first", "First", "", -1.),
        command("navigation", "Navigation", "navigation", 0.),
        command("localized", "AAA", "primary", 0.),
        command("middle", "BBB", "primary", 0.),
        command("tie", "BBB", "primary", 0.),
    ];
    assert_eq!(ids(&menu_actions(&registry, MenuId::Test, &no_context())), ["navigation", "-", "localized", "middle", "tie", "-", "first", "a", "z"]);
}

/// menuService.test.ts 'getMenuActions omits empty submenus and observes nested registrations'.
#[test]
fn a_submenu_of_empty_submenus_is_left_out() {
    let submenu = |menu, submenu, title: &str| MenuItem { menu, entry: Entry::Submenu { submenu, toggled: None }, title: title.into(), group: String::new(), order: 0., when: None };
    let mut registry = vec![submenu(MenuId::Test, MenuId::ThemesSubMenu, "Submenu"), submenu(MenuId::ThemesSubMenu, MenuId::MenubarHelpMenu, "Nested submenu")];
    assert_eq!(menu_actions(&registry, MenuId::Test, &no_context()), []);
    let mut nested = command("nested", "Nested", "", 0.);
    nested.menu = MenuId::MenubarHelpMenu;
    registry.push(nested);
    assert_eq!(ids(&menu_actions(&registry, MenuId::Test, &no_context())), ["Submenu"]);
}

/// menuService.test.ts 'getMenuActions reevaluates enablement, toggled state and alternate actions'.
#[test]
fn enablement_toggled_title_and_alt_precondition() {
    let mut item = command("command", "Command", "", 0.);
    let toggled = Some(Toggled { condition: Expr::has("toggled"), title: Some("Toggled".into()) });
    let alt = Some(Alt { id: "alt".into(), title: "Alternate".into(), precondition: Some(Expr::has("enabled")) });
    item.entry = Entry::Command { id: "command".into(), precondition: Some(Expr::has("enabled")), toggled, alt };
    let registry = [item];
    let snapshot = |enabled: bool, toggled: bool| {
        let context = HashMap::from([("enabled".to_string(), json!(enabled)), ("toggled".to_string(), json!(toggled))]);
        let actions = menu_actions(&registry, MenuId::Test, &context);
        let [MenuAction::Command { enabled, checked, title, alt, .. }] = actions.as_slice() else { unreachable!() };
        (*enabled, *checked, title.clone(), alt.is_some())
    };
    assert_eq!(snapshot(true, false), (true, false, "Command".to_string(), true));
    assert_eq!(snapshot(false, true), (false, true, "Toggled".to_string(), false));
}

/// menuService.test.ts 'getMenuActions evaluates submenu toggle state without checking its children'.
#[test]
fn submenu_toggle_state_leaves_its_children_unchecked() {
    let submenu = MenuItem { menu: MenuId::Test, entry: Entry::Submenu { submenu: MenuId::ThemesSubMenu, toggled: Some(Expr::has("submenuActive")) }, title: "Submenu".into(), group: String::new(), order: 0., when: None };
    let mut child = command("child", "Child", "", 0.);
    child.menu = MenuId::ThemesSubMenu;
    let registry = [submenu, child];
    let snapshot = |active: bool| {
        let context = HashMap::from([("submenuActive".to_string(), json!(active))]);
        let actions = menu_actions(&registry, MenuId::Test, &context);
        let [MenuAction::Submenu { checked, actions: children, .. }] = actions.as_slice() else { unreachable!() };
        let [MenuAction::Command { checked: child_checked, .. }] = children.as_slice() else { unreachable!() };
        (*checked, *child_checked)
    };
    assert_eq!(snapshot(false), (false, false));
    assert_eq!(snapshot(true), (true, false));
}

#[test]
fn submenus_without_actions_are_left_out() {
    let submenu = |title: &str, group: &str| MenuItem { menu: MenuId::Test, entry: Entry::Submenu { submenu: MenuId::ThemesSubMenu, toggled: None }, title: title.into(), group: group.into(), order: 0., when: None };
    let mut theme = command("theme", "Color Theme", "", 1.);
    theme.menu = MenuId::ThemesSubMenu;
    let empty = MenuItem { entry: Entry::Submenu { submenu: MenuId::MenubarHelpMenu, toggled: None }, ..submenu("Empty", "2") };
    let actions = menu_actions(&[submenu("Themes", "1"), empty, theme], MenuId::Test, &no_context());
    assert_eq!(actions, [MenuAction::Submenu { title: "Themes".into(), actions: vec![MenuAction::Command { id: "theme".into(), title: "Color Theme".into(), enabled: true, checked: false, alt: None }], checked: false }]);
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
            "commands": [
                { "command": "a.open", "title": "Open Preview", "enablement": "ready" },
                { "command": "a.alt", "title": "Open to the Side" },
                { "command": "a.lockedAlt", "title": "Locked", "enablement": "ready" }
            ],
            "menus": {
                "explorer/context": [
                    { "command": "a.open", "alt": "a.alt", "group": "navigation@2", "when": "resourceLangId == markdown" },
                    { "command": "a.missing", "group": "navigation" },
                    { "command": "a.alt", "group": "4_search@x" },
                    { "command": "a.alt", "alt": "a.lockedAlt", "group": "5_locked", "when": "resourceLangId == markdown" }
                ],
                "notAMenu": [{ "command": "a.open" }]
            }
        }),
    )];
    let items = extension_menus(&extensions);
    let summary: Vec<_> = items.iter().map(|item| (item.menu, item.title.as_str(), item.group.as_str(), item.order)).collect();
    assert_eq!(
        summary,
        [(MenuId::ExplorerContext, "Open Preview", "navigation", 2.), (MenuId::ExplorerContext, "Open to the Side", "4_search", 0.), (MenuId::ExplorerContext, "Open to the Side", "5_locked", 0.)]
    );
    let context = HashMap::from([("resourceLangId".to_string(), json!("markdown"))]);
    let actions = menu_actions(&items, MenuId::ExplorerContext, &context);
    let open = MenuAction::Command { id: "a.open".into(), title: "Open Preview".into(), enabled: false, checked: false, alt: Some(("a.alt".into(), "Open to the Side".into())) };
    assert_eq!(actions[0], open);
    // An alt whose command's enablement does not hold is not offered (`getActionForAltMode`).
    assert!(matches!(&actions[4], MenuAction::Command { id, alt: None, .. } if id == "a.alt"));
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
    assert_eq!(actions, [MenuAction::Submenu { title: "More".into(), actions: vec![one], checked: false }]);
}

/// clipboard.ts and menusExtensionPoint.ts: the github extension's share items fill the Share submenus of the
/// Explorer and editor context menus; Copy As stays hidden while nothing fills it.
#[test]
fn the_github_share_items_fill_the_share_submenus() {
    let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let github = exthost::scan::scan_extension(&app_root.join("extensions/github"), true).unwrap();
    let mut registry = workbench_menus();
    registry.extend(extension_menus(&[github]));
    let context = HashMap::from([("github.hasGitHubRepo".to_string(), json!(true)), ("resourceScheme".to_string(), json!("file")), ("editorTextFocus".to_string(), json!(true))]);
    let share = |menu| {
        let actions = menu_actions(&registry, menu, &context);
        let submenu = actions.iter().find(|action| matches!(action, MenuAction::Submenu { title, .. } if title == "Share")).cloned();
        (ids(&actions).contains(&"Copy As"), submenu)
    };
    let link = |id: &str, title: &str| Some(MenuAction::Submenu { title: "Share".into(), actions: vec![MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked: false, alt: None }], checked: false });
    assert_eq!(share(MenuId::ExplorerContext), (false, link("github.copyVscodeDevLinkWithoutRange", "Copy vscode.dev Link")));
    assert_eq!(share(MenuId::EditorContext), (false, link("github.copyVscodeDevLink", "Copy vscode.dev Link")));
    assert_eq!(share(MenuId::EditorTitleContext), (false, link("github.copyVscodeDevLinkWithoutRange", "Copy vscode.dev Link")));
    assert!(!ids(&menu_actions(&registry, MenuId::ExplorerContext, &no_context())).contains(&"Share"), "no repository, no Share");
}

/// The activity bar's Manage menu: the reference's items whose commands the port has, in its groups.
#[test]
fn the_manage_menu_lists_the_global_activity_items() {
    let actions = menu_actions(&workbench_menus(), MenuId::GlobalActivity, &no_context());
    assert_eq!(ids(&actions), ["workbench.action.showCommands", "-", "workbench.action.openGlobalKeybindings", "Themes"]);
}

#[test]
fn mnemonic_menu_label_drops_mnemonics() {
    assert_eq!(mnemonic_menu_label("Save A&&ll"), "Save All");
    assert_eq!(mnemonic_menu_label("Open (&&O)"), "Open ");
    assert_eq!(mnemonic_menu_label("Fish & Chips"), "Fish & Chips");
}
