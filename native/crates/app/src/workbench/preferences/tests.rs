use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::editor::Position;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{color_themes, load_theme};
use crate::workbench::Workbench;

fn workbench<'a>(cx: &'a mut TestAppContext, name: &str) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let scan = |name: &str| scan_extension(&app_root.join("extensions").join(name), true).unwrap();
    let themes = [scan("theme-defaults")];
    let theme = load_theme(&themes, "Dark 2026").unwrap();
    let languages = Languages::new(&[scan("json")]);
    let dir = std::env::temp_dir().join(format!("code-gpui-preferences-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::core(), dir.join("User"), None)));
    let (sender, _messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, languages, settings, HostSync::new(sender), cx);
        workbench.color_themes = color_themes(&themes).unwrap();
        workbench
    });
    (workbench, cx, dir)
}

fn run(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, command: &str, args: Vec<Value>) {
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command(command, args, window, cx));
    cx.run_until_parked();
}

fn active_text_and_cursor(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (String, Position) {
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (editor.document().lines.join("\n"), editor.selections[0].head)
    })
}

#[gpui::test]
fn open_settings_json_creates_the_file_and_reveals_a_language_block_for_editing(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "edit");
    let settings_file = dir.join("User/settings.json");

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": "[json]", "edit": true } })]);

    assert_eq!(std::fs::read_to_string(&settings_file).unwrap(), "{\n}", "emptyEditableSettingsContent on disk");
    let (text, cursor) = active_text_and_cursor(&workbench, cx);
    assert_eq!(text, "{\n    \"[json]\": {\n        \n    }\n}");
    assert_eq!(cursor, Position::new(2, 8));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_setting_query_reveals_its_key_or_its_value(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "query");
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), "{\n\t\"editor.tabSize\": 2,\n}").unwrap();

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "query": "@id:editor.tabSize" })]);
    assert_eq!(active_text_and_cursor(&workbench, cx).1, Position::new(1, 1));

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": "editor.tabSize", "edit": true } })]);
    assert_eq!(active_text_and_cursor(&workbench, cx), ("{\n\t\"editor.tabSize\": 2,\n}".to_string(), Position::new(1, 20)));

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": "editor.unknownSetting", "edit": true } })]);
    assert_eq!(active_text_and_cursor(&workbench, cx).0, "{\n\t\"editor.tabSize\": 2,\n}", "an unregistered key is not added");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn an_open_settings_file_marks_its_unknown_settings_until_it_closes(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "unknown");
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), "{\n\t\"editor.tabSize\": 2,\n\t\"editor.unknownSetting\": true\n}").unwrap();
    let markers = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| {
            let markers = workbench.diagnostics.service.read(crate::markers::ReadOptions { owner: Some("UnsupportedSettingsRenderer"), ..Default::default() });
            markers.iter().map(|marker| (marker.resource.clone(), marker.message.clone(), marker.start_line, marker.end_column)).collect::<Vec<_>>()
        })
    };

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![]);
    assert_eq!(markers(&workbench, cx), Vec::new(), "the renderer waits 200ms");
    cx.executor().advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    let resource = crate::editor::uri_string("file", &dir.join("User/settings.json"));
    assert_eq!(markers(&workbench, cx), vec![(resource, "Unknown Configuration Setting".to_string(), 3, 31)]);

    run(&workbench, cx, "workbench.action.closeActiveEditor", vec![]);
    cx.executor().advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(markers(&workbench, cx), Vec::new(), "dispose removes the markers");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_raw_default_settings_open_read_only(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "raw");

    run(&workbench, cx, "workbench.action.openRawDefaultSettings", vec![]);

    let (uri, first_line, language, readonly) = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        let document = &editor.document();
        (document.uri_string(), document.lines[0].clone(), document.language_id.clone(), workbench.active_editor_readonly())
    });
    assert_eq!((uri.as_str(), first_line.as_str(), language.as_str()), ("vscode://defaultsettings/defaultSettings.jsonc", "{", "jsonc"));
    assert_eq!(readonly, Some(super::super::readonly::Readonly::Provider));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn the_default_keybindings_file_lists_the_defaults_then_the_unbound_commands(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "keybindings");
    workbench.update(cx, |workbench, cx| workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx));

    run(&workbench, cx, "workbench.action.openDefaultKeybindingsFile", vec![]);

    let (name, lines) = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (workbench.editor_name(editor), editor.document().lines.clone())
    });
    assert_eq!(name, "Default Keybindings");
    assert_eq!(lines[..2], ["// Override key bindings by placing them into your key bindings file.", "["]);
    assert!(lines.contains(&"{ \"key\": \"cmd+,\",                 \"command\": \"workbench.action.openSettings\" },".to_string()));
    let when = lines.iter().position(|line| line.trim_start().starts_with("\"when\": ")).unwrap();
    assert!(lines[when].starts_with("                                     \"when\": \"") && lines[when - 1].ends_with(","));
    let unbound = lines.iter().position(|line| line == "// Here are other available commands: ").unwrap();
    assert_eq!(lines[unbound - 3..unbound], ["]", "", ""]);
    assert!(lines[unbound + 1..].iter().all(|line| line.starts_with("// - ")));
    assert!(lines.contains(&"// - workbench.action.openRawDefaultSettings".to_string()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn export_default_keybindings_writes_the_core_then_the_built_in_extension_rules(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "export");
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let markdown = scan_extension(&app_root.join("extensions/markdown-language-features"), true).unwrap();
    let written = workbench.update(cx, |workbench, cx| {
        workbench.load_keybindings(crate::keybindings::default_keybindings(std::slice::from_ref(&markdown)), dir.join("User/keybindings.json"), cx);
        workbench.extensions = vec![markdown];
        workbench.export_default_keybindings(&dir.join("doc")).unwrap()
    });

    assert_eq!(written, dir.join("doc/doc.keybindings.osx.json"));
    let text = std::fs::read_to_string(&written).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "[", "no header");
    let preview = lines.iter().position(|line| line.contains("\"markdown.showPreviewToSide\"")).unwrap();
    let last_core = lines.iter().rposition(|line| line.contains("\"workbench.action.openSettings\"")).unwrap();
    assert!(preview > last_core, "the built-in extension rules come after the core rules");
    assert!(text.contains("\n]\n\n\n// Here are other available commands: \n// - "));
    // Core rules keep their platform keys, as the reference's export (/tmp/kbref on the Studio) writes them.
    assert!(text.contains(r#""when": "isMacNative && openFolderWorkspaceSupport""#), "isMacNative stays");
    assert!(text.contains(r#""when": "inChatInput && isLinux && !quickChatHasFocus || inChatInput && isWindows && !quickChatHasFocus""#));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn workspace_settings_need_a_folder(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "workspace");
    run(&workbench, cx, "workbench.action.openWorkspaceSettingsFile", vec![]);
    assert!(workbench.read_with(cx, |workbench, _| workbench.editors.active().is_none()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn pick_workspace_folder_answers_the_accepted_folder_as_the_api_expects(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "pick-folder");
    let mut workspace = config::workspace::Workspace::folder(dir.join("one"));
    workspace.folders.push(config::workspace::WorkspaceFolder { path: dir.join("two"), name: "two".into() });
    workbench.update(cx, |workbench, _| workbench.settings.lock().unwrap().workspace = Some(workspace));
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_workbench.pickWorkspaceFolder", vec![json!({ "placeHolder": "Pick one" })], window, cx));
    let crate::commands::Outcome::Pending(mut picked) = outcome else { panic!("the pick waits for the quick pick") };
    cx.run_until_parked();
    let placeholder = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().and_then(|input| input.placeholder.clone()));
    assert_eq!(placeholder.as_deref(), Some("Pick one"));
    run(&workbench, cx, "quickInput.next", vec![]);
    run(&workbench, cx, "workbench.action.acceptSelectedQuickOpenItem", vec![]);
    let answer = picked.try_recv().unwrap().expect("answered").unwrap();
    assert_eq!(answer, json!({ "uri": crate::editor::uri_value("file", &dir.join("two")), "name": "two", "index": 1 }));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn pick_workspace_folder_without_folders_answers_nothing(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "pick-no-folder");
    let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_workbench.pickWorkspaceFolder", vec![], window, cx));
    assert!(matches!(outcome, crate::commands::Outcome::Ready(Ok(Value::Null))));
    std::fs::remove_dir_all(dir).unwrap();
}

/// preferencesService.ts `doOpenSplitJSON` and splitSettingsEditorInput.test.ts: with `workbench.settings.useSplitJSON`
/// settings.json opens as the primary side of a side by side editor whose secondary side is the read-only default
/// settings of its target; a workbench action from the secondary side acts on the group, and closing the editor
/// closes both sides.
#[gpui::test]
fn split_json_opens_the_default_settings_beside_settings_json(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "split");
    workbench.update(cx, |workbench, _| {
        workbench.settings.lock().unwrap().update_value("workbench.settings.useSplitJSON", Some(&json!(true)), config::service::Target::User, None, &[]).unwrap();
    });

    run(&workbench, cx, "workbench.action.openSettingsJson", vec![]);

    let side = workbench.update(cx, |workbench, _| {
        assert_eq!(workbench.editors.active().unwrap().document().path, dir.join("User/settings.json"));
        let side = workbench.editors.side_group().expect("a side by side editor");
        let (editors, _) = workbench.editors.group_tabs(side);
        let secondary = editors[0].document();
        assert_eq!((secondary.scheme.as_str(), secondary.path.as_path()), ("vscode", Path::new("//defaultsettings/settings.json")));
        assert_eq!(secondary.lines[0], "[{");
        drop(secondary);
        assert!(!workbench.editors.group_ids_mru().contains(&side) && !workbench.editors.has_several_groups());
        workbench.activate_group(side);
        side
    });
    run(&workbench, cx, "workbench.action.closeActiveEditor", vec![]);
    workbench.read_with(cx, |workbench, _| {
        assert!(workbench.editors.side_by_sides.is_empty());
        assert!(workbench.editors.group(side).is_none() && workbench.editors.id != side);
    });
    std::fs::remove_dir_all(dir).unwrap();
}

/// sideBySideEditor.ts `lastFocusedSide`: a workbench action from the read-only side acts on the group and the side
/// keeps the focus after it; Go to Line acts on the side's code editor (`activeTextEditorControl`).
#[gpui::test]
fn a_workbench_action_from_the_secondary_side_leaves_it_active(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "split-side");
    workbench.update(cx, |workbench, _| {
        workbench.settings.lock().unwrap().update_value("workbench.settings.useSplitJSON", Some(&json!(true)), config::service::Target::User, None, &[]).unwrap();
    });
    run(&workbench, cx, "workbench.action.openSettingsJson", vec![]);
    let side = workbench.update(cx, |workbench, _| {
        let side = workbench.editors.side_group().expect("a side by side editor");
        workbench.activate_group(side);
        side
    });

    run(&workbench, cx, "workbench.action.toggleSplitEditorInGroupLayout", vec![]);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.id, side, "the side keeps the focus"));
    run(&workbench, cx, "workbench.action.gotoLine", vec![]);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.editors.active().unwrap().document().scheme, "vscode", "Go to Line on the read-only side"));
    std::fs::remove_dir_all(dir).unwrap();
}

/// editorTypePicker.ts with preferencesContribution.ts's registration: settings.json offers the split settings editor
/// (the longer glob first) and the text editor; picking the text editor turns the side by side editor into
/// settings.json's text editor, and the split settings editor brings the default settings back beside it.
#[gpui::test]
fn the_editor_type_picker_switches_settings_json_between_split_and_text(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "editor-type");
    workbench.update(cx, |workbench, _| {
        workbench.settings.lock().unwrap().update_value("workbench.settings.useSplitJSON", Some(&json!(true)), config::service::Target::User, None, &[]).unwrap();
    });
    run(&workbench, cx, "workbench.action.openSettingsJson", vec![]);
    workbench.update_in(cx, |workbench, window, cx| {
        let available = workbench.available_editor_types().expect("two editors for settings.json");
        let ids: Vec<&str> = available.editors.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, ["workbench.editorinputs.sidebysideEditorInput", "default"]);
        assert_eq!((available.current.as_str(), available.pattern.as_str()), ("workbench.editorinputs.sidebysideEditorInput", "**/settings.json"));
        drop(workbench.reopen_editor_type(&available, "default", window, cx));
        assert!(workbench.editors.side_by_sides.is_empty());
        assert_eq!(workbench.editors.active().unwrap().document().path, dir.join("User/settings.json"));
        let available = workbench.available_editor_types().expect("still two editors");
        assert_eq!(available.current, "default");
        drop(workbench.reopen_editor_type(&available, "workbench.editorinputs.sidebysideEditorInput", window, cx));
        assert!(workbench.editors.side_group().is_some());
    });
    std::fs::remove_dir_all(dir).unwrap();
}

/// preferencesRenderers.ts `EditSettingRenderer`: the cursor's enum setting gets the pencil on its value's line, and
/// a value picked from its menu is written into the file and selected.
#[gpui::test]
fn the_edit_setting_pencil_writes_the_picked_value(cx: &mut TestAppContext) {
    let (workbench, cx, dir) = workbench(cx, "pencil");
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), "{\n\t\"editor.wordWrap\": \"off\",\n\t\"editor.tabSize\": 2\n}").unwrap();
    run(&workbench, cx, "workbench.action.openSettingsJson", vec![]);
    let pencil = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, line: usize| {
        workbench.update(cx, |workbench, _| {
            workbench.editors.active_mut().unwrap().set_selections(vec![crate::editor::Selection::caret(Position::new(line, 1))]);
            workbench.edit_setting_line(workbench.editors.active().unwrap())
        })
    };
    assert_eq!(pencil(&workbench, cx, 1), Some(2));
    assert_eq!(pencil(&workbench, cx, 2), None, "a number has no values to pick");

    workbench.update_in(cx, |workbench, window, cx| {
        let model = workbench.editors.active().unwrap().model_id;
        workbench.show_edit_setting_menu(model, 2, gpui::Point::default(), window, cx);
    });
    run(&workbench, cx, "editSetting.value.1", vec![]);
    let expected = "{\n\t\"editor.wordWrap\": \"on\",\n\t\"editor.tabSize\": 2\n}";
    assert_eq!(std::fs::read_to_string(dir.join("User/settings.json")).unwrap(), expected);
    let selection = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection());
    assert_eq!((selection.anchor, selection.head), (Position::new(1, 20), Position::new(1, 24)), "the value with its quotes");
    std::fs::remove_dir_all(dir).unwrap();
}
