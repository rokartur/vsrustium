use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use exthost::scan::scan_extension;
use files::watcher::{Change, ChangeType};
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

use crate::editor::Position;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

/// A workbench over a temp folder whose user settings are `user_settings`.
fn workbench_in<'a>(name: &str, user_settings: &str, cx: &'a mut TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut VisualTestContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-text-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), user_settings).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let settings = Arc::new(Mutex::new(settings));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.skip_dialogs = true;
        workbench
    });
    (dir, workbench, cx)
}

fn run(workbench: &gpui::Entity<Workbench>, id: &str, args: Vec<serde_json::Value>, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, args, window, cx)));
    cx.run_until_parked();
}

fn open(workbench: &gpui::Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(path.to_path_buf(), true, window, cx));
}

fn type_text(workbench: &gpui::Entity<Workbench>, text: &str, cx: &mut VisualTestContext) {
    run(workbench, "type", vec![json!({ "text": text })], cx);
}

fn active_text(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (String, bool) {
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        (editor.document().lines.join("\n"), editor.is_dirty())
    })
}

#[gpui::test]
fn new_untitled_files_are_named_by_their_first_line(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("untitled", "{}", cx);
    run(&workbench, "workbench.action.files.newUntitledFile", Vec::new(), cx);
    run(&workbench, "workbench.action.files.newUntitledFile", Vec::new(), cx);
    let names = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().map(|editor| workbench.editor_name(editor)).collect::<Vec<_>>());
    assert_eq!(names(cx), ["Untitled-1", "Untitled-2"]);
    type_text(&workbench, "  hello   world", cx);
    assert_eq!(names(cx), ["Untitled-1", "hello world"]);
    std::fs::write(dir.join("User/settings.json"), r#"{ "workbench.editor.untitled.labelFormat": "name" }"#).unwrap();
    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));
    assert_eq!(names(cx), ["Untitled-1", "Untitled-2"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn an_untitled_file_with_a_path_saves_as_that_file_in_its_editor(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("save-as", "{}", cx);
    let target = dir.join("new.txt");
    let uri = json!({ "scheme": "untitled", "path": target.to_string_lossy() });
    workbench.update_in(cx, |workbench, window, cx| workbench.api_open_editor(&[uri], window, cx)).unwrap();
    type_text(&workbench, "abc", cx);
    run(&workbench, "workbench.action.files.save", Vec::new(), cx);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "abc");
    let (schemes, cursor, dirty) = workbench.read_with(cx, |workbench, _| {
        let schemes: Vec<String> = workbench.editors.editors.iter().chain(&workbench.editors.models).map(|editor| editor.document().scheme.clone()).collect();
        let editor = workbench.editors.active().unwrap();
        (schemes, editor.selection().head, editor.is_dirty())
    });
    assert_eq!(schemes, ["file"], "the untitled model is gone, the file's shows in its place");
    assert_eq!(cursor, Position::new(0, 3), "the cursor stays where it was");
    assert!(!dirty);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn revert_reads_the_file_again_without_writing_it(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("revert", "{}", cx);
    let file = dir.join("a.txt");
    std::fs::write(&file, "one\ntwo").unwrap();
    open(&workbench, &file, cx);
    type_text(&workbench, "x", cx);
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    run(&workbench, "workbench.action.files.revert", Vec::new(), cx);
    assert_eq!(active_text(&workbench, cx), ("one\ntwo".to_string(), false));
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), modified, "a revert does not write the file");
    run(&workbench, "undo", Vec::new(), cx);
    assert_eq!(active_text(&workbench, cx), ("xone\ntwo".to_string(), true), "updateModel is one undoable edit");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_file_changed_on_disk_fails_the_save_until_overwrite(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("conflict", "{}", cx);
    let file = dir.join("a.txt");
    std::fs::write(&file, "one").unwrap();
    open(&workbench, &file, cx);
    type_text(&workbench, "x", cx);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, "changed elsewhere").unwrap();
    run(&workbench, "workbench.action.files.save", Vec::new(), cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed elsewhere");
    let message = workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.first().map(|item| (item.message.text(), item.primary.iter().map(|action| action.label.clone()).collect::<Vec<_>>())));
    let expected = "Failed to save 'a.txt': The content of the file is newer. Please compare your version with the file contents or overwrite the content of the file with your changes.";
    assert_eq!(message, Some((expected.to_string(), vec!["Compare".to_string(), "Overwrite".to_string()])));
    let uri = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().uri());
    run(&workbench, "workbench.files.action.saveIgnoreModifiedSince", vec![uri], cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xone");
    assert!(workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.is_empty()), "the message closes once the model saves");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Compare opens the file on disk against the model with the help message; Save again from there shows the help,
/// the title's accept action overwrites the file, reopens the file's editor and closes the conflict editor.
#[gpui::test]
fn compare_opens_the_save_conflict_editor(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("conflict-compare", "{}", cx);
    let file = dir.join("a.txt");
    std::fs::write(&file, "one").unwrap();
    open(&workbench, &file, cx);
    type_text(&workbench, "x", cx);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, "changed elsewhere").unwrap();
    run(&workbench, "workbench.action.files.save", Vec::new(), cx);
    let uri = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().uri());
    run(&workbench, "workbench.files.action.resolveConflict", vec![uri.clone()], cx);
    let messages = |cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.iter().map(|item| item.message.text()).collect::<Vec<_>>());
    let help = super::super::save_conflict::CONFLICT_EDITOR_HELP.to_string();
    assert_eq!(messages(cx), [help.clone()], "the help replaces the save error of the same model");
    workbench.update_in(cx, |workbench, window, cx| {
        let diff = workbench.editors.active_diff().expect("the conflict editor");
        assert_eq!(diff.label.as_deref(), Some("a.txt (in file) \u{2194} a.txt (in VSRustium) - Resolve save conflict"));
        assert_eq!((diff.original.scheme.as_str(), diff.modified.scheme.as_str()), ("conflictResolution", "file"));
        let original = workbench.editors.model("conflictResolution", &file).unwrap();
        assert_eq!(original.document().lines.join("\n"), "changed elsewhere");
        assert_eq!(workbench.key_context(window).get("saveConflictResolutionContext"), Some(&json!(true)));
        let _ = cx;
    });
    run(&workbench, "workbench.action.files.save", Vec::new(), cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed elsewhere");
    assert!(messages(cx).iter().all(|message| *message == help), "a save from the conflict editor shows the help again");
    run(&workbench, "workbench.files.action.acceptLocalChanges", vec![uri], cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xone");
    assert!(messages(cx).is_empty());
    workbench.read_with(cx, |workbench, _| {
        assert!(workbench.editors.active_diff().is_none(), "the conflict editor closed");
        assert_eq!(workbench.editors.active().unwrap().document().scheme, "file");
    });
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn save_without_formatting_skips_the_save_participants(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("without-formatting", r#"{ "files.trimTrailingWhitespace": true }"#, cx);
    let file = dir.join("a.txt");
    std::fs::write(&file, "one").unwrap();
    open(&workbench, &file, cx);
    run(&workbench, "cursorEnd", Vec::new(), cx);
    type_text(&workbench, "  ", cx);
    run(&workbench, "workbench.action.files.saveWithoutFormatting", Vec::new(), cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one  ");
    type_text(&workbench, " ", cx);
    run(&workbench, "workbench.action.files.save", Vec::new(), cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn clean_models_follow_their_file_and_deleted_files_orphan_them(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("on-disk", "{}", cx);
    let file = dir.join("a.txt");
    std::fs::write(&file, "one").unwrap();
    open(&workbench, &file, cx);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, "two\nlines").unwrap();
    let changed = |kind| vec![Change { path: file.clone(), kind }];
    workbench.update(cx, |workbench, cx| workbench.files_changed(changed(ChangeType::Updated), cx));
    assert_eq!(active_text(&workbench, cx), ("two\nlines".to_string(), false));

    // textFileEditorModelManager.ts: never reload dirty models.
    run(&workbench, "cursorTop", Vec::new(), cx);
    type_text(&workbench, "x", cx);
    std::fs::write(&file, "three").unwrap();
    workbench.update(cx, |workbench, cx| workbench.files_changed(changed(ChangeType::Updated), cx));
    assert_eq!(active_text(&workbench, cx), ("xtwo\nlines".to_string(), true));

    std::fs::remove_file(&file).unwrap();
    workbench.update(cx, |workbench, cx| workbench.files_changed(changed(ChangeType::Deleted), cx));
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    assert!(workbench.read_with(cx, |workbench, _| workbench.text_files.orphaned.contains(&file)));
    std::fs::write(&file, "back").unwrap();
    workbench.update(cx, |workbench, cx| workbench.files_changed(changed(ChangeType::Added), cx));
    assert!(workbench.read_with(cx, |workbench, _| !workbench.text_files.orphaned.contains(&file)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn files_associations_change_the_language_of_models_without_a_picked_one(cx: &mut TestAppContext) {
    let (dir, workbench, cx) = workbench_in("associations", "{}", cx);
    let (a, b) = (dir.join("a.foo"), dir.join("b.foo"));
    std::fs::write(&a, "a").unwrap();
    std::fs::write(&b, "b").unwrap();
    open(&workbench, &a, cx);
    open(&workbench, &b, cx);
    workbench.update(cx, |workbench, cx| workbench.set_active_language("plaintext", cx));
    std::fs::write(dir.join("User/settings.json"), r#"{ "files.associations": { "*.foo": "markdown" } }"#).unwrap();
    workbench.update(cx, |workbench, cx| workbench.reload_changed_settings_files(cx));
    let languages = workbench.read_with(cx, |workbench, _| workbench.editors.editors.iter().map(|editor| editor.document().language_id.clone()).collect::<Vec<_>>());
    assert_eq!(languages, ["markdown", "plaintext"], "b's language was picked");
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn a_write_locked_file_saves_after_overwrite(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let (dir, workbench, cx) = workbench_in("locked", "{}", cx);
    let file = dir.join("a.txt");
    std::fs::write(&file, "one").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    open(&workbench, &file, cx);
    type_text(&workbench, "x", cx);
    run(&workbench, "workbench.action.files.save", Vec::new(), cx);
    let message = workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.first().map(|item| (item.message.text(), item.primary.iter().map(|action| action.label.clone()).collect::<Vec<_>>())));
    let expected = "Failed to save 'a.txt': File is read-only. Select 'Overwrite' to attempt to make it writeable.";
    assert_eq!(message, Some((expected.to_string(), vec!["Overwrite".to_string(), "Save As...".to_string(), "Revert".to_string()])));
    let uri = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().uri());
    run(&workbench, "workbench.files.action.unlock", vec![uri], cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xone");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o200, 0o200);
    std::fs::remove_dir_all(dir).unwrap();
}
