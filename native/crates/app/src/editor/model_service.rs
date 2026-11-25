//! The closed-model undo cache of modelService.ts (with workbench/services/model's `file` scheme): a model of a
//! file closed with undo or redo elements keeps them, and a model created again for that file with the same
//! text takes them back with its version ids.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use config::configuration::Overrides;
use config::service::Settings;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::edit::History;
use super::{OpenEditor, Position, Replace};

/// `ModelService.MAX_MEMORY_FOR_CLOSED_FILES_UNDO_STACK`.
const MAX_MEMORY_FOR_CLOSED_FILES_UNDO_STACK: usize = 20 * 1024 * 1024;
/// `DefaultModelSHA1Computer.MAX_MODEL_SIZE`.
const MAX_MODEL_SIZE: usize = 10 * 1024 * 1024;

/// `DisposedModelInfo`.
struct DisposedModel {
    history: History,
    version_id: u32,
    digest: [u8; 32],
    time: Instant,
    heap_size: usize,
}

#[derive(Default)]
pub struct DisposedModels {
    models: HashMap<String, DisposedModel>,
    heap_size: usize,
    /// Read for `files.restoreUndoStack`; without it the stacks are kept.
    pub settings: Option<Arc<Mutex<Settings>>>,
}

impl DisposedModels {
    /// `_onWillDispose`: the model's undo and redo elements are kept when its scheme maintains them
    /// (`_schemaShouldMaintainUndoRedoElements`), they fit in the cache and the text is small enough to hash.
    pub fn insert(&mut self, mut model: OpenEditor) {
        if !self.should_restore_undo_stack() || !matches!(model.document().scheme.as_str(), "file" | "fake-fs") {
            return;
        }
        let Some(heap_size) = model.text_model().history.heap_size() else { return };
        let text = model.document().lines.join(model.document().eol);
        if heap_size > MAX_MEMORY_FOR_CLOSED_FILES_UNDO_STACK || text.len() > MAX_MODEL_SIZE {
            return;
        }
        self.ensure_heap_size(MAX_MEMORY_FOR_CLOSED_FILES_UNDO_STACK - heap_size);
        let version_id = model.text_model().version_id;
        let disposed = DisposedModel { history: std::mem::take(&mut model.text_model_mut().history), version_id, digest: digest(&text), time: Instant::now(), heap_size };
        self.heap_size += heap_size;
        if let Some(replaced) = self.models.insert(model.document().uri_string(), disposed) {
            self.heap_size -= replaced.heap_size;
        }
    }

    /// `_createModelData`: a new model of a disposed one's resource takes back its elements and version ids when
    /// the text is the same; either way the disposed entry goes.
    pub fn restore(&mut self, model: &mut OpenEditor) {
        let Some(disposed) = self.models.remove(&model.document().uri_string()) else { return };
        self.heap_size -= disposed.heap_size;
        let text = model.document().lines.join(model.document().eol);
        if text.len() > MAX_MODEL_SIZE || digest(&text) != disposed.digest {
            return;
        }
        model.text_model_mut().history = disposed.history;
        model.text_model_mut().version_id = disposed.version_id;
        // textFileEditorModel.ts `updateSavedVersionId` after the model is created: it matches the disk.
        model.mark_saved();
    }

    /// `_shouldRestoreUndoStack`: `files.restoreUndoStack` unless it is not a boolean.
    fn should_restore_undo_stack(&self) -> bool {
        let Some(settings) = &self.settings else { return true };
        settings.lock().unwrap().get_value(Some("files.restoreUndoStack"), &Overrides::default()) != Some(json!(false))
    }

    /// A boolean setting of `resource` that is on unless it is `false`.
    pub fn setting_enabled(&self, key: &str, resource: &Path) -> bool {
        let Some(settings) = &self.settings else { return true };
        settings.lock().unwrap().get_value(Some(key), &Overrides { resource: Some(resource), override_identifier: None }) != Some(json!(false))
    }

    /// `_ensureDisposedModelsHeapSize`: the oldest entries go first.
    fn ensure_heap_size(&mut self, max_heap_size: usize) {
        while self.heap_size > max_heap_size {
            let oldest = self.models.iter().min_by_key(|(_, model)| model.time).map(|(uri, _)| uri.clone()).expect("a heap size above 0 has entries");
            let removed = self.models.remove(&oldest).expect("just found");
            self.heap_size -= removed.heap_size;
        }
    }
}

/// `ModelService._computeEdits`: one edit from `old` to `new` lines over the lines between their common prefix
/// and suffix, its text joined with `eol`; `None` when they are equal.
pub fn compute_edits(old: &[String], new: &[String], eol: &str) -> Option<Replace> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if old.len() == new.len() && prefix == old.len() {
        return None;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    let chars = |line: &String| line.chars().count();
    if suffix > 0 {
        let mut text = String::new();
        for line in &new[prefix..new.len() - suffix] {
            text.push_str(line);
            text.push_str(eol);
        }
        return Some(Replace::new(Position::new(prefix, 0), Position::new(old.len() - suffix, 0), text));
    }
    if prefix > 0 {
        let text = match new.len() > prefix {
            true => format!("{eol}{}", new[prefix..].join(eol)),
            false => String::new(),
        };
        return Some(Replace::new(Position::new(prefix - 1, chars(&old[prefix - 1])), Position::new(old.len() - 1, chars(&old[old.len() - 1])), text));
    }
    Some(Replace::new(Position::new(0, 0), Position::new(old.len() - 1, chars(&old[old.len() - 1])), new.join(eol)))
}

/// `computeSHA1`; SHA-256 here, the only digest the port links.
fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{EditReason, EditorGroup, TextDocument};
    use crate::languages::Languages;
    use std::path::Path;

    fn file(text: &str) -> TextDocument {
        let mut document = TextDocument::untitled(Path::new("/w/a.txt"), text, None, &Languages::new(&[]), &crate::default_settings().lock().unwrap());
        document.scheme = "file".into();
        document
    }

    #[test]
    fn a_reopened_file_with_the_same_text_keeps_its_undo_stack() {
        // modelService.test.ts 'maintains undo for same resource and same content'.
        let mut group = EditorGroup::default();
        group.open(file("text"), false);
        let editor = &mut group.editors[0];
        editor.execute_edits(vec![(Position::new(0, 0), Position::new(0, 0), "new ".into(), false)], None, EditReason::Unknown("test"));
        editor.mark_saved();
        let version = editor.text_model().version_id;
        group.close(0);
        group.open(file("new text"), false);
        let editor = &mut group.editors[0];
        assert_eq!(editor.text_model().version_id, version);
        assert!(!editor.is_dirty());
        assert!(editor.undo().is_some());
        assert_eq!(editor.document().lines, ["text"]);
        assert!(editor.is_dirty());
    }

    #[test]
    fn a_reopened_file_with_other_text_starts_a_new_undo_stack() {
        // modelService.test.ts 'does not maintain undo for same resource and different content'.
        let mut group = EditorGroup::default();
        group.open(file("text"), false);
        group.editors[0].execute_edits(vec![(Position::new(0, 0), Position::new(0, 0), "new ".into(), false)], None, EditReason::Unknown("test"));
        group.close(0);
        group.open(file("other"), false);
        assert!(group.editors[0].undo().is_none());
        group.close(0);
        group.open(file("new text"), false);
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
        group.editors[0].execute_edits(vec![(Position::new(0, 0), Position::new(0, 0), "new ".into(), false)], None, EditReason::Unknown("test"));
        group.close(0);
        group.disposed.settings = None;
        group.open(file("new text"), false);
        assert!(group.editors[0].undo().is_none());
    }

    /// modelService.test.ts `assertComputeEdits`: the edit applied to the first lines gives the second.
    fn assert_compute_edits(old: &[&str], new: &[&str]) {
        let old: Vec<String> = old.iter().map(|line| line.to_string()).collect();
        let new: Vec<String> = new.iter().map(|line| line.to_string()).collect();
        let Some(edit) = compute_edits(&old, &new, "\n") else { return assert_eq!(old, new) };
        let text = old.join("\n");
        let offset = |position: Position| old[..position.line].iter().map(|line| line.len() + 1).sum::<usize>() + old[position.line].chars().take(position.column).map(char::len_utf8).sum::<usize>();
        let applied = format!("{}{}{}", &text[..offset(edit.start)], edit.text, &text[offset(edit.end)..]);
        assert_eq!(applied, new.join("\n"), "{old:?} to {new:?}");
    }

    #[test]
    fn compute_edits_turns_one_text_into_another() {
        let lines = ["This is line one", "and this is line number two", "it is followed by #3", "and finished with the fourth."];
        let owned = |lines: &[&str]| lines.iter().map(|line| line.to_string()).collect::<Vec<_>>();
        assert!(compute_edits(&owned(&lines), &owned(&lines), "\n").is_none(), "_computeEdits no change");
        let changed = ["This is line One", lines[1], lines[2], lines[3]];
        let edit = compute_edits(&owned(&lines), &owned(&changed), "\n").unwrap();
        assert_eq!((edit.start, edit.end, edit.text.as_str()), (Position::new(0, 0), Position::new(1, 0), "This is line One\n"), "_computeEdits first line changed");
        assert_compute_edits(&["pram", "okctibad", "pjuwtemued", "knnnm", "u", ""], &["tcnr", "rxwlicro", "vnzy", "", "", "pjzcogzur", "ptmxyp", "dfyshia", "pee", "ygg"]);
        assert_compute_edits(&["", "itls", "hrilyhesv", ""], &["vdl", "", "tchgz", "bhx", "nyl"]);
        assert_compute_edits(&["ubrbrcv", "wv", "xodspybszt", "s", "wednjxm", "fklajt", "fyfc", "lvejgge", "rtpjlodmmk", "arivtgmjdm"], &["s", "qj", "tu", "ur", "qerhjjhyvx", "t"]);
        assert_compute_edits(&["ig", "kh", "hxegci", "smvker", "pkdmjjdqnv", "vgkkqqx", "", "jrzeb"], &["yk", ""]);
        assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 2", "line 5", "line 3"]);
        assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 2", "line 3", "line 4"]);
        assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 0", "line 1", "line 2", "line 3"]);
        assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 7", "line 3"]);
        assert_compute_edits(&["line 1", "line 2", "line 3"], &["line 1", "line 3"]);
        assert_compute_edits(&["line 1", "line 2", "line 3", "line 4", "line 5"], &["line 0", "line 1", "replace line 2", "line 3", "line 5"]);
        assert_compute_edits(&["a", "b", "c"], &["a", "b"]);
    }
}
