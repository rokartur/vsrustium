use super::*;

fn command(id: &str, label: &str, category: Option<&str>) -> CommandPick {
    CommandPick { id: id.into(), label: label.into(), category: category.map(Into::into) }
}

fn commands() -> Vec<CommandPick> {
    vec![
        command("workbench.action.files.save", "File: Save", Some("File")),
        command("workbench.action.files.saveFiles", "File: Save All Files", Some("File")),
        command("workbench.action.closeAllEditors", "View: Close All Editors", Some("View")),
        command("workbench.action.reloadWindow", "Developer: Reload Window", Some("Developer")),
        command("undo", "Undo", None),
        command("editor.action.clipboardCopyAction", "Copy", None),
    ]
}

fn labels(entries: &[PaletteEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            PaletteEntry::Separator(label) => format!("--{label}"),
            PaletteEntry::Pick(pick) => pick.label.clone(),
        })
        .collect()
}

#[test]
fn empty_filter_sorts_by_label_with_developer_commands_last() {
    let entries = command_picks(&commands(), "", &CommandsHistory::new(DEFAULT_HISTORY_LENGTH));
    assert_eq!(labels(&entries), ["Copy", "File: Save", "File: Save All Files", "Undo", "View: Close All Editors", "Developer: Reload Window"]);
}

#[test]
fn filter_matches_words_and_substrings_with_highlights() {
    let history = CommandsHistory::new(DEFAULT_HISTORY_LENGTH);
    let entries = command_picks(&commands(), "fsa", &history);
    assert_eq!(labels(&entries), ["File: Save", "File: Save All Files"]);
    let entries = command_picks(&commands(), "fsaf", &history);
    assert_eq!(labels(&entries), ["File: Save All Files"]);
    let PaletteEntry::Pick(pick) = &entries[0] else { panic!("a pick") };
    assert_eq!(pick.label_highlights, [0..1, 6..8, 15..16]);

    let entries = command_picks(&commands(), "save", &history);
    assert_eq!(labels(&entries), ["File: Save", "File: Save All Files"]);
    let PaletteEntry::Pick(pick) = &entries[0] else { panic!("a pick") };
    assert_eq!(pick.label_highlights, [6..10]);
}

#[test]
fn exact_command_id_matches() {
    let entries = command_picks(&commands(), "undo", &CommandsHistory::new(DEFAULT_HISTORY_LENGTH));
    assert_eq!(labels(&entries), ["Undo"]);
    let entries = command_picks(&commands(), "workbench.action.closeAllEditors", &CommandsHistory::new(DEFAULT_HISTORY_LENGTH));
    assert_eq!(labels(&entries), ["View: Close All Editors"]);
}

#[test]
fn recently_used_come_first_under_separators() {
    let mut history = CommandsHistory::new(DEFAULT_HISTORY_LENGTH);
    history.push("undo");
    history.push("workbench.action.files.save");
    let entries = command_picks(&commands(), "", &history);
    assert_eq!(labels(&entries)[..5], ["--recently used", "File: Save", "Undo", "--other commands", "Copy"]);
    let PaletteEntry::Pick(pick) = &entries[1] else { panic!("a pick") };
    assert!(pick.recently_used);

    history.remove("workbench.action.files.save");
    history.push("undo");
    assert_eq!(labels(&command_picks(&commands(), "", &history))[..3], ["--recently used", "Undo", "--other commands"]);
    history.clear();
    assert_eq!(labels(&command_picks(&commands(), "", &history))[0], "Copy");
}

#[test]
fn history_keeps_the_most_recent_up_to_its_limit() {
    let mut history = CommandsHistory::new(2);
    history.push("a");
    history.push("b");
    history.push("a");
    history.push("c");
    assert_eq!(history.peek("b"), None);
    assert_eq!(history.peek("a"), Some(3));
    assert_eq!(history.peek("c"), Some(4));
}

#[test]
fn similar_commands_by_tf_idf_follow_the_matches() {
    let commands = vec![command("a", "Close Window", None), command("b", "Reopen Closed Editor", None), command("c", "Window: Zoom", None)];
    let entries = command_picks(&commands, "shut window", &CommandsHistory::new(DEFAULT_HISTORY_LENGTH));
    assert_eq!(labels(&entries), ["--similar commands", "Close Window", "Window: Zoom"]);
}

#[test]
fn duplicate_labels_show_their_command_ids() {
    let commands = vec![command("one.run", "Run", None), command("two.run", "Run", None), command("stop", "Stop", None)];
    let entries = command_picks(&commands, "", &CommandsHistory::new(DEFAULT_HISTORY_LENGTH));
    let descriptions: Vec<Option<String>> = entries
        .iter()
        .map(|entry| match entry {
            PaletteEntry::Pick(pick) => pick.description.clone(),
            PaletteEntry::Separator(_) => None,
        })
        .collect();
    assert_eq!(descriptions, [Some("one.run".into()), Some("two.run".into()), None]);
}

#[test]
fn locale_compare_orders_like_english_collation() {
    let mut labels = vec!["b", "B", "a1", "a:", "a b", "é", "e", "f"];
    labels.sort_by(|a, b| locale_compare(a, b));
    assert_eq!(labels, ["a b", "a:", "a1", "b", "B", "e", "é", "f"]);
}

#[test]
fn strip_icons_like_icon_labels_test() {
    assert_eq!(strip_icons("Hello World"), "Hello World");
    assert_eq!(strip_icons("$(Hello World"), "$(Hello World");
    assert_eq!(strip_icons("$(Hello) World"), " World");
    assert_eq!(strip_icons("$(Hello) W$(oi)rld"), " Wrld");
}

fn extension(description: Value) -> ScannedExtension {
    ScannedExtension { id: "test.ext".into(), location: "/ext".into(), description }
}

#[test]
fn menu_lists_contributed_commands_and_honours_command_palette_when() {
    let description = serde_json::json!({ "contributes": {
        "commands": [
            { "command": "ext.hello", "title": "Hello", "category": "Ext" },
            { "command": "ext.hidden", "title": "Hidden" },
            { "command": "ext.disabled", "title": "Disabled", "enablement": "ext.ready" },
            { "command": "ext.titled", "title": { "value": "Titled", "original": "Titled" } },
            { "command": "ext.uncategorized", "title": "Uncategorized", "category": "" }
        ],
        "menus": { "commandPalette": [
            { "command": "ext.hidden", "when": "false" },
            { "command": "ext.unknown" }
        ] }
    } });
    let menu = menu(&[extension(description)]);
    let context = std::collections::HashMap::new();
    let ids: Vec<String> = enabled_commands(&menu, &context).into_iter().filter(|pick| pick.id.starts_with("ext.")).map(|pick| pick.label).collect();
    assert_eq!(ids, ["Ext: Hello", "Titled", "Uncategorized"]);

    let ready = std::collections::HashMap::from([("ext.ready".to_string(), Value::Bool(true))]);
    let picks = enabled_commands(&menu, &ready);
    assert!(picks.iter().any(|pick| pick.id == "ext.disabled"));
    assert!(picks.iter().any(|pick| pick.label == "File: Save All Files" && pick.category.as_deref() == Some("File")));
    assert!(!menu.iter().any(|item| item.id == "ext.unknown"));
}
