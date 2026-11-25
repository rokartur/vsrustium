use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use super::{OPEN_COMMAND, column_resize_validation, context_menu_actions, copied_entry, record_chord, row_height, table_columns};
use crate::host_sync::HostSync;
use crate::keybindings::parse_keybinding;
use crate::keybindings_editor::{Entry, EntryMatches, Row, Source};
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn entry(keybinding: &str, when: &str, is_default: bool, command_label: &str) -> Entry {
    let row = Row {
        keybinding: parse_keybinding(keybinding),
        command: "workbench.action.files.save".into(),
        command_label: command_label.into(),
        args: None,
        when: when.into(),
        is_default,
        source: if is_default { Source::System } else { Source::User },
    };
    Entry { row, matches: EntryMatches::default() }
}

fn enabled_titles(actions: &[MenuAction]) -> Vec<String> {
    let title = |action: &MenuAction| match action {
        MenuAction::Command { title, enabled: true, .. } => title.clone(),
        MenuAction::Command { title, .. } => format!("({title})"),
        _ => "---".into(),
    };
    actions.iter().map(title).collect()
}

#[test]
fn a_recorded_third_chord_starts_over() {
    let [cmd_k, cmd_s, cmd_j] = ["cmd+k", "cmd+s", "cmd+j"].map(|key| parse_keybinding(key)[0]);
    let mut chords = Vec::new();
    for chord in [cmd_k, cmd_s, cmd_j] {
        record_chord(&mut chords, chord);
    }
    assert_eq!(chords, [cmd_j]);
}

#[test]
fn the_row_menu_enables_what_the_keybinding_allows() {
    let bound_default = context_menu_actions(&entry("cmd+s", "", true, "Save"));
    assert_eq!(
        enabled_titles(&bound_default),
        ["Copy", "Copy Command ID", "Copy Command Title", "---", "Change Keybinding...", "Add Keybinding...", "---", "Remove Keybinding", "(Reset Keybinding)", "---", "Change When Expression", "---", "Show Same Keybindings"]
    );
    let unbound_user = context_menu_actions(&entry("", "", false, ""));
    assert_eq!(
        enabled_titles(&unbound_user),
        ["Copy", "Copy Command ID", "(Copy Command Title)", "---", "Add Keybinding...", "---", "(Remove Keybinding)", "Reset Keybinding", "---", "(Change When Expression)", "---", "(Show Same Keybindings)"]
    );
}

#[test]
fn copy_writes_the_user_settings_form_with_the_when_only_when_set() {
    assert_eq!(copied_entry(&entry("cmd+k cmd+s", "", true, "Save")), "{\n  \"key\": \"cmd+k cmd+s\",\n  \"command\": \"workbench.action.files.save\"\n}");
    assert!(copied_entry(&entry("cmd+s", "editorFocus", true, "Save")).ends_with("\"when\": \"editorFocus\"\n}"));
}

#[test]
fn rows_grow_for_a_matched_id_and_columns_share_the_width_by_weight() {
    let mut matched = entry("cmd+s", "", true, "Save");
    assert_eq!(row_height(&matched), 24.);
    matched.matches.command_id = Some(vec![0..4]);
    assert_eq!(row_height(&matched), 40.);
    // Source drops to its 120 minimum and When gives the rest of the 40 the actions column takes.
    let mut columns = table_columns();
    columns.layout(1040.);
    assert_eq!(columns.sizes(), [40., 312., 208., 360., 120.]);
    columns.layout(2000.);
    assert_eq!(columns.sizes(), [40., 600., 400., 700., 260.]);
}

#[test]
fn a_column_width_must_be_an_integer_percentage() {
    let invalid_type = Some("Please enter an integer.".to_string());
    let invalid_range = Some("Please enter a number greater than 0 and less than or equal to 100.".to_string());
    for (value, message) in [("", None), ("50", None), (" 50%", None), ("0", None), ("100", None), ("0x10", None), ("abc", invalid_type.clone()), (" ", invalid_type), ("101", invalid_range.clone()), ("-1", invalid_range.clone()), ("0x65", invalid_range.clone()), ("99999999999999999999", invalid_range)] {
        assert_eq!(column_resize_validation(value), message, "{value:?}");
    }
}

#[gpui::test]
fn resize_column_sets_the_picked_column_to_a_percentage_of_the_table(cx: &mut TestAppContext) {
    let (workbench, cx, _) = keybindings_editor_window(cx, "resize-column", "");
    cx.simulate_keystrokes("cmd-k cmd-s");
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("keybindings.editor.focusKeybindings", Vec::new(), window, cx)));
    cx.run_until_parked();
    let width = workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.columns.size());

    // The When column, then 50.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("list.resizeColumn", Vec::new(), window, cx)));
    cx.run_until_parked();
    cx.simulate_input("when");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    let prompt = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().and_then(|input| input.prompt.clone()));
    assert_eq!(prompt.as_deref(), Some("Please enter a width in percentage for the 'When' column."));
    cx.simulate_input("150");
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_some_and(|input| input.validation_message.is_some())), "an invalid value keeps the input open");
    cx.simulate_keystrokes("backspace backspace backspace");
    cx.simulate_input("50");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sizes = workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.columns.sizes());
    assert_eq!(sizes[3], (width / 2.).round(), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<f32>(), width);
    assert!(workbench.update_in(cx, |workbench, window, _| workbench.keybindings_editor.table_focus.is_focused(window)), "focus goes back to the table");
}

fn keybindings_editor_window<'a>(cx: &'a mut TestAppContext, name: &str, content: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, std::path::PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-keybindings-editor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let file = dir.join("User/keybindings.json");
    std::fs::write(&file, content).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings.clone(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.quick_access.menu = crate::command_palette::menu(&[]);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), file.clone(), cx);
        workbench.skip_dialogs = true;
        workbench
    });
    (workbench, cx, file)
}

fn filter(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
}

fn shown_commands(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.entries.iter().map(|entry| entry.row.command.clone()).collect())
}

fn search(workbench: &gpui::Entity<Workbench>, query: &str, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(OPEN_COMMAND, vec![json!(query)], window, cx)));
    filter(cx);
}

#[gpui::test]
fn keys_define_a_keybinding_that_lands_in_keybindings_json(cx: &mut TestAppContext) {
    let (workbench, cx, file) = keybindings_editor_window(cx, "define", "");
    cx.simulate_keystrokes("cmd-k cmd-s");
    assert!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.open));
    assert!(shown_commands(&workbench, cx).len() > 100, "every command is listed");

    // Cmd+Down goes from the search to the table, Enter there defines the focused row's keys. Escape in the
    // define widget closes the modal, as in 1.147.0: its capture listener runs closeModalEditor first.
    cx.simulate_input("copy");
    filter(cx);
    cx.simulate_keystrokes("cmd-down");
    assert!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.open), "Cmd+Down keeps the modal");
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.define.is_some()), "Enter in the table opens the define widget");
    cx.simulate_keystrokes("escape");
    assert!(workbench.read_with(cx, |workbench, _| !workbench.keybindings_editor.open));
    cx.simulate_keystrokes("cmd-k cmd-s");
    filter(cx);
    assert!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.search.value.is_empty()), "the modal opens with an empty search");

    // Typed text filters 300ms later. Escape closes the modal even with a search value: closeModalEditor's
    // weight (WorkbenchContrib + 10) outranks clearSearchResults' (WorkbenchContrib).
    cx.simulate_input("clear command history");
    filter(cx);
    assert_eq!(shown_commands(&workbench, cx), ["workbench.action.clearCommandHistory"]);
    cx.simulate_keystrokes("escape");
    assert!(workbench.read_with(cx, |workbench, _| !workbench.keybindings_editor.open));

    // Configure Keybinding's query focuses the one row; Enter defines its keys, Enter writes them.
    search(&workbench, "@command:workbench.action.clearCommandHistory", cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.selected), Some(0));
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.define.is_some()), "Enter opens the define widget");
    cx.simulate_keystrokes("cmd-k cmd-j");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.define.as_ref().unwrap().chords.clone()), parse_keybinding("cmd+k cmd+j"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("\"key\": \"cmd+k cmd+j\"") && written.contains("\"command\": \"workbench.action.clearCommandHistory\""), "{written}");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.entries[0].row.keybinding.clone()), parse_keybinding("cmd+k cmd+j"));

    // Cmd+Backspace removes it; the list keys move through the rows, Escape in the table closes the modal.
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert!(!std::fs::read_to_string(&file).unwrap().contains("cmd+k cmd+j"));
    search(&workbench, "workbench.action", cx);
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("keybindings.editor.focusKeybindings", Vec::new(), window, cx)));
    cx.simulate_keystrokes("home down down up");
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.selected), Some(1));
    cx.simulate_keystrokes("escape");
    assert!(workbench.read_with(cx, |workbench, _| !workbench.keybindings_editor.open));
}

#[gpui::test]
fn writes_wait_until_an_open_keybindings_json_is_saved(cx: &mut TestAppContext) {
    let content = "// Place your key bindings in this file to override the defaults\n[\n  { \"key\": \"cmd+k cmd+j\", \"command\": \"workbench.action.clearCommandHistory\" }\n]";
    let (workbench, cx, file) = keybindings_editor_window(cx, "dirty", content);
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(file.clone(), true, window, cx));
    cx.simulate_input("x");
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().is_dirty()));

    // The bound row's Remove Keybinding refuses, the file keeps the keybinding.
    search(&workbench, "@command:workbench.action.clearCommandHistory", cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.keybindings_editor.entries[0].row.keybinding.clone()), parse_keybinding("cmd+k cmd+j"));
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), content);
}

#[gpui::test]
fn the_modal_blocks_the_command_palette_key_but_not_the_command(cx: &mut TestAppContext) {
    let (workbench, cx, _) = keybindings_editor_window(cx, "palette", "");
    cx.simulate_keystrokes("cmd-k cmd-s");
    cx.simulate_keystrokes("cmd-shift-p");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none()), "modalEditorPart.ts stops workbench.action.showCommands");

    // The menu bar's View > Command Palette... runs the command itself; the quick input draws above the modal.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.showCommands", Vec::new(), window, cx)));
    cx.run_until_parked();
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_some() && workbench.keybindings_editor.open));
}
