use std::path::Path;

use serde_json::json;

use super::*;
use crate::editor::{EditorGroup, TextDocument};
use crate::languages::Languages;

fn file(text: &str) -> TextDocument {
    let mut document = TextDocument::untitled(Path::new("/w/a.txt"), text, None, &Languages::new(&[]), &crate::default_settings().lock().unwrap());
    document.scheme = "file".into();
    document
}

fn long_text() -> String {
    let mut text = String::new();
    for block in 0..40 {
        text.push_str(&format!("block {block}\n    a\n    b\n"));
    }
    text
}

#[test]
fn a_reopened_file_takes_back_its_cursors_folds_and_scroll() {
    let mut group = EditorGroup::default();
    group.viewport = Viewport { width: 800., height: 200., char_width: 8., ..Viewport::default() };
    group.open(file(&long_text()), false);
    let viewport = group.viewport;
    let editor = &mut group.editors[0];
    editor.click_fold_control(3, true, false, false);
    editor.set_selections(vec![Selection { anchor: Position::new(30, 1), head: Position::new(31, 3) }]);
    editor.set_scroll(10. * viewport.line_height + 5., 0., viewport);
    let (folding, scroll, selections) = (editor.folding.get_memento(&editor.document.lines), editor.scroll.top, editor.selections.clone());
    assert!(folding.is_some());
    group.close(0);

    group.open(file(&long_text()), false);
    let editor = &group.editors[0];
    assert_eq!(editor.selections, selections);
    assert_eq!(editor.folding.get_memento(&editor.document.lines), folding);
    assert_eq!(editor.scroll.top, scroll);
}

#[test]
fn view_states_round_trip_through_the_memento_oldest_first() {
    let mut states = ViewStates::default();
    for index in 0..LIMIT + 1 {
        states.save(&format!("file:///{index}"), json!({ "n": index }));
    }
    assert_eq!(states.load("file:///0"), None, "the oldest goes past the limit");
    assert_eq!(states.load("file:///1"), Some(json!({ "n": 1 })));
    let mut restored = ViewStates::from_memento(&states.to_memento());
    let memento: serde_json::Value = serde_json::from_str(&states.to_memento()).unwrap();
    assert_eq!(memento["textEditorViewState"][LIMIT - 1], json!(["file:///1", { "0": { "n": 1 } }]), "a load makes it the newest");
    assert_eq!(restored.load("file:///2"), Some(json!({ "n": 2 })));
}

#[test]
fn restore_view_state_off_does_not_restore_a_new_editor() {
    let user = std::env::temp_dir().join(format!("vsrustium-restore-view-state-{}", std::process::id()));
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("settings.json"), r#"{ "workbench.editor.restoreViewState": false }"#).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None);
    std::fs::remove_dir_all(&user).unwrap();
    let mut group = EditorGroup::default();
    group.disposed.settings = Some(std::sync::Arc::new(std::sync::Mutex::new(settings)));
    group.open(file("a\nb\nc"), false);
    group.editors[0].set_selections(vec![Selection::caret(Position::new(2, 1))]);
    group.close(0);
    group.open(file("a\nb\nc"), false);
    assert_eq!(group.editors[0].selections, [Selection::default()]);
}
