use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use config::service::Settings;
use config::workspace::Workspace;

use super::compute_edits;
use crate::editor::{EditReason, EditorGroup, Position, TextDocument};
use crate::languages::Languages;

fn file(text: &str) -> TextDocument {
    let mut document = TextDocument::untitled(Path::new("/w/a.txt"), text, None, &Languages::new(&[]), &crate::default_settings().lock().unwrap());
    document.scheme = "file".into();
    document
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

const FOUR_LINES: [&str; 4] = ["This is line one", "and this is line number two", "it is followed by #3", "and finished with the fourth."];

/// modelService.test.ts `assertComputeEdits`: the edit applied to the first lines gives the second.
fn assert_compute_edits(old: &[&str], new: &[&str]) {
    let (old, new) = (lines(old), lines(new));
    let Some(edit) = compute_edits(&old, &new, "\n") else { return assert_eq!(old, new) };
    let text = old.join("\n");
    let offset = |position: Position| old[..position.line].iter().map(|line| line.len() + 1).sum::<usize>() + old[position.line].chars().take(position.column).map(char::len_utf8).sum::<usize>();
    let applied = format!("{}{}{}", &text[..offset(edit.start)], edit.text, &text[offset(edit.end)..]);
    assert_eq!(applied, new.join("\n"), "{old:?} to {new:?}");
}

/// "text" becomes "text1" and is saved: closing a dirty model keeps it (`canDispose`) where upstream's `destroyModel` disposes it.
fn insert_one_and_save(group: &mut EditorGroup) {
    group.editors[0].execute_edits(vec![(Position::new(0, 4), Position::new(0, 4), "1".into(), false)], None, EditReason::Unknown("test"));
    group.editors[0].mark_saved();
}

#[test]
fn the_eol_setting_of_a_models_folder_applies_to_it() {
    let dir = std::env::temp_dir().join(format!("vsrustium-eol-root-{}", std::process::id()));
    let root = dir.join("myroot");
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::create_dir_all(root.join(".vscode")).unwrap();
    std::fs::create_dir_all(dir.join("other")).unwrap();
    std::fs::write(dir.join("User/settings.json"), r#"{ "files.eol": "\n" }"#).unwrap();
    std::fs::write(root.join(".vscode/settings.json"), r#"{ "files.eol": "\r\n" }"#).unwrap();
    // Two roots: a single folder's settings are workspace settings and reach every resource.
    std::fs::write(dir.join("w.code-workspace"), r#"{ "folders": [{ "path": "myroot" }, { "path": "other" }] }"#).unwrap();
    let settings = Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), Some(Workspace::from_file(dir.join("w.code-workspace"))));
    let eol = |scheme: &str, path: PathBuf| TextDocument::from_text(scheme, &path, "farboo", "utf8".into(), None, &Languages::new(&[]), &settings).eol;
    assert_eq!(eol("untitled", PathBuf::from("Untitled-1")), "\n");
    assert_eq!(eol("file", root.join("myfile.txt")), "\r\n");
    assert_eq!(eol("file", dir.join("other/myfile.txt")), "\n");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn compute_edits_of_the_same_lines_is_no_edit() {
    assert!(compute_edits(&lines(&FOUR_LINES), &lines(&FOUR_LINES), "\n").is_none());
}

#[test]
fn compute_edits_of_a_changed_first_line_replaces_that_line() {
    let changed = ["This is line One", FOUR_LINES[1], FOUR_LINES[2], FOUR_LINES[3]];
    let edit = compute_edits(&lines(&FOUR_LINES), &lines(&changed), "\n").unwrap();
    assert_eq!((edit.start, edit.end, edit.text.as_str()), (Position::new(0, 0), Position::new(1, 0), "This is line One\n"));
}

#[test]
fn compute_edits_of_a_changed_eol_is_no_edit() {
    // update_model splits the new text into lines and pushes its EOL apart, so only the lines reach compute_edits.
    let (new, eol) = crate::editor::split_lines(&FOUR_LINES.join("\r\n"));
    assert_eq!(eol, Some("\r\n"));
    assert!(compute_edits(&lines(&FOUR_LINES), &new, "\n").is_none());
}

#[test]
fn compute_edits_with_a_changed_eol_and_changed_lines_joins_them_with_the_new_eol() {
    let changed = ["This is line One", FOUR_LINES[1], "It is followed by #3", FOUR_LINES[3]];
    let edit = compute_edits(&lines(&FOUR_LINES), &lines(&changed), "\r\n").unwrap();
    let text = ["This is line One", "and this is line number two", "It is followed by #3", ""].join("\r\n");
    assert_eq!((edit.start, edit.end, edit.text.as_str()), (Position::new(0, 0), Position::new(3, 0), text.as_str()));
}

#[test]
fn compute_edits_with_a_changed_eol_and_a_new_last_line_inserts_one_eol() {
    let old = ["package main", "func foo() {", "}"];
    let edit = compute_edits(&lines(&old), &lines(&["package main", "func foo() {", "}", ""]), "\r\n").unwrap();
    assert_eq!((edit.start, edit.end, edit.text.as_str()), (Position::new(2, 1), Position::new(2, 1), "\r\n"));
}

#[test]
fn compute_edits_turns_generated_text_1_into_another() {
    assert_compute_edits(&["pram", "okctibad", "pjuwtemued", "knnnm", "u", ""], &["tcnr", "rxwlicro", "vnzy", "", "", "pjzcogzur", "ptmxyp", "dfyshia", "pee", "ygg"]);
}

#[test]
fn compute_edits_turns_generated_text_2_into_another() {
    assert_compute_edits(&["", "itls", "hrilyhesv", ""], &["vdl", "", "tchgz", "bhx", "nyl"]);
}

#[test]
fn compute_edits_turns_generated_text_3_into_another() {
    assert_compute_edits(&["ubrbrcv", "wv", "xodspybszt", "s", "wednjxm", "fklajt", "fyfc", "lvejgge", "rtpjlodmmk", "arivtgmjdm"], &["s", "qj", "tu", "ur", "qerhjjhyvx", "t"]);
}

#[test]
fn compute_edits_turns_generated_text_4_into_another() {
    assert_compute_edits(&["ig", "kh", "hxegci", "smvker", "pkdmjjdqnv", "vgkkqqx", "", "jrzeb"], &["yk", ""]);
}

#[test]
fn compute_edits_inserts_in_the_middle_of_the_document() {
    assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 2", "line 5", "line 3"]);
}

#[test]
fn compute_edits_inserts_at_the_end_of_the_document() {
    assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 2", "line 3", "line 4"]);
}

#[test]
fn compute_edits_inserts_at_the_beginning_of_the_document() {
    assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 0", "line 1", "line 2", "line 3"]);
}

#[test]
fn compute_edits_replaces_lines() {
    assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 7", "line 3"]);
}

#[test]
fn compute_edits_deletes_lines() {
    assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 3"]);
    assert_compute_edits(&["a", "b", "c"], &["a", "b"]);
}

#[test]
fn compute_edits_inserts_replaces_and_deletes_lines() {
    assert_compute_edits(&["line 1", "line 2", "line 3", "line 4", "line 5"], &["line 0", "line 1", "replace line 2", "line 3", "line 5"]);
}

#[test]
fn a_reopened_file_with_the_same_text_keeps_its_undo_stack() {
    let mut group = EditorGroup::default();
    group.open(file("text"), false);
    insert_one_and_save(&mut group);
    assert_eq!(group.editors[0].document.lines, ["text1"]);
    group.close(0);
    group.open(file("text1"), false);
    let editor = &mut group.editors[0];
    assert!(!editor.is_dirty());
    assert!(editor.undo().is_some());
    assert_eq!(editor.document.lines, ["text"]);
    assert!(editor.is_dirty());
}

#[test]
fn a_reopened_file_with_the_same_text_keeps_its_version_id_and_alternative_version_id() {
    let mut group = EditorGroup::default();
    group.open(file("text"), false);
    insert_one_and_save(&mut group);
    let (version, alternative) = (group.editors[0].version_id, group.editors[0].alternative_version_id());
    group.close(0);
    group.open(file("text1"), false);
    assert_eq!((group.editors[0].version_id, group.editors[0].alternative_version_id()), (version, alternative));
}

#[test]
fn a_reopened_file_with_other_text_starts_a_new_undo_stack() {
    let mut group = EditorGroup::default();
    group.open(file("text"), false);
    insert_one_and_save(&mut group);
    group.close(0);
    group.open(file("text2"), false);
    assert!(group.editors[0].undo().is_none());
    assert_eq!(group.editors[0].document.lines, ["text2"]);
    group.close(0);
    group.open(file("text1"), false);
    assert!(group.editors[0].undo().is_none(), "the entry went with the first create");
}

#[test]
fn restore_undo_stack_off_keeps_nothing() {
    let mut group = EditorGroup::default();
    let user = std::env::temp_dir().join(format!("vsrustium-restore-undo-{}", std::process::id()));
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("settings.json"), r#"{ "files.restoreUndoStack": false }"#).unwrap();
    let settings = Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None);
    std::fs::remove_dir_all(&user).unwrap();
    group.disposed.settings = Some(Arc::new(Mutex::new(settings)));
    group.open(file("text"), false);
    insert_one_and_save(&mut group);
    group.close(0);
    group.disposed.settings = None;
    group.open(file("text1"), false);
    assert!(group.editors[0].undo().is_none());
}
