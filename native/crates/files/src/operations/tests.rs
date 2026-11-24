//! bulkFileEdits.test.ts and the event cases of workingCopyFileService.test.ts.

use std::collections::BTreeMap;

use super::*;
use crate::{FILE_NOT_FOUND, Stat, provider_error};

/// A file system of plain entries that records the calls the upstream mocks record.
#[derive(Default)]
struct Recorder {
    entries: BTreeMap<String, (u32, Vec<u8>)>,
    log: Vec<String>,
    events: Vec<(FileOperation, Vec<SourceTarget>, &'static str)>,
}

impl FileSystem for Recorder {
    fn capabilities(&mut self, _scheme: &str) -> Option<u32> {
        Some(crate::FILE_FOLDER_COPY)
    }

    fn stat(&mut self, resource: &Resource) -> Result<Stat> {
        if resource.path == "/" {
            return Ok(Stat { file_type: DIRECTORY, ctime: 0, mtime: 0, size: 0, permissions: None });
        }
        let (file_type, bytes) = self.entries.get(&resource.path).ok_or_else(|| provider_error(FILE_NOT_FOUND, resource.path.clone()))?;
        Ok(Stat { file_type: *file_type, ctime: 0, mtime: 0, size: bytes.len() as u64, permissions: None })
    }

    fn read_dir(&mut self, resource: &Resource) -> Result<Vec<(String, u32)>> {
        let prefix = format!("{}/", resource.path.trim_end_matches('/'));
        let children = self.entries.iter().filter(|(path, _)| path.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('/')));
        Ok(children.map(|(path, (file_type, _))| (path[prefix.len()..].to_owned(), *file_type)).collect())
    }

    fn read_file(&mut self, resource: &Resource) -> Result<Vec<u8>> {
        self.log.push(format!("read {}", resource.path));
        Ok(self.entries[&resource.path].1.clone())
    }

    fn write_file(&mut self, resource: &Resource, content: &[u8], _create: bool, _overwrite: bool) -> Result<()> {
        self.log.push(format!("create {} {content:?}", resource.path));
        self.entries.insert(resource.path.clone(), (FILE, content.to_vec()));
        Ok(())
    }

    fn mkdir(&mut self, resource: &Resource) -> Result<()> {
        self.log.push(format!("createFolder {}", resource.path));
        self.entries.insert(resource.path.clone(), (DIRECTORY, Vec::new()));
        Ok(())
    }

    fn delete(&mut self, resource: &Resource, _recursive: bool, _use_trash: bool) -> Result<()> {
        self.log.push(format!("delete {}", resource.path));
        self.entries.retain(|path, _| path != &resource.path && !path.starts_with(&format!("{}/", resource.path)));
        Ok(())
    }

    fn rename(&mut self, from: &Resource, to: &Resource, _overwrite: bool) -> Result<()> {
        self.log.push(format!("move {} {}", from.path, to.path));
        let entry = self.entries.remove(&from.path).unwrap();
        self.entries.insert(to.path.clone(), entry);
        Ok(())
    }

    fn copy(&mut self, from: &Resource, to: &Resource, _overwrite: bool) -> Result<()> {
        self.log.push(format!("copy {} {}", from.path, to.path));
        let entry = self.entries[&from.path].clone();
        self.entries.insert(to.path.clone(), entry);
        Ok(())
    }
}

impl WorkingCopyFiles for Recorder {
    fn participate(&mut self, operation: FileOperation, files: &[SourceTarget], _undo: UndoInfo) {
        self.events.push((operation, files.to_vec(), "will"));
    }

    fn did_run(&mut self, operation: FileOperation, files: &[SourceTarget]) {
        self.events.push((operation, files.to_vec(), "did"));
    }

    fn enable_trash(&mut self) -> bool {
        false
    }

    fn empty_file(&mut self, _resource: &Resource) -> Vec<u8> {
        panic!("Undo must restore the original contents, not create an empty file")
    }
}

/// `applyDelete`: deletes entries of `types`, undoes the delete and returns the logged calls.
fn apply_delete(types: &[u32], options: FileEditOptions) -> Vec<String> {
    let mut fs = Recorder::default();
    fs.entries.insert("/workspace".into(), (DIRECTORY, Vec::new()));
    let mut edits = Vec::new();
    for (index, file_type) in types.iter().enumerate() {
        let path = format!("/workspace/entry-{index}");
        fs.entries.insert(path.clone(), (*file_type, vec![index as u8, 128, 255]));
        let folder = file_type & DIRECTORY != 0;
        edits.push(FileEdit::Delete { old: Resource::file(path), options: options.over(true, folder), undoes_create: false });
    }
    let mut undo = apply(&mut fs, edits, None).unwrap();
    undo.reverse(&mut fs).unwrap();
    fs.log
}

#[test]
fn delete_skips_undo_contents_for_non_regular_files() {
    assert_eq!(apply_delete(&[0], FileEditOptions::default()), ["delete /workspace/entry-0"]);
}

#[test]
fn delete_preserves_undo_contents_for_regular_files() {
    assert_eq!(apply_delete(&[FILE], FileEditOptions::default()), ["read /workspace/entry-0", "delete /workspace/entry-0", "create /workspace/entry-0 [0, 128, 255]"]);
}

#[test]
fn undo_restores_only_regular_files_in_a_mixed_deletion() {
    assert_eq!(
        apply_delete(&[FILE, 0, FILE], FileEditOptions::default()),
        [
            "read /workspace/entry-0",
            "read /workspace/entry-2",
            "delete /workspace/entry-0",
            "delete /workspace/entry-1",
            "delete /workspace/entry-2",
            "create /workspace/entry-0 [0, 128, 255]",
            "create /workspace/entry-2 [2, 128, 255]",
        ]
    );
}

#[test]
fn delete_does_not_restore_non_regular_files_when_the_folder_option_is_set() {
    assert_eq!(apply_delete(&[0], FileEditOptions { folder: Some(true), ..FileEditOptions::default() }), ["delete /workspace/entry-0"]);
}

#[test]
fn delete_preserves_undo_for_empty_folders() {
    assert_eq!(apply_delete(&[DIRECTORY], FileEditOptions::default()), ["delete /workspace/entry-0", "createFolder /workspace/entry-0"]);
}

#[test]
fn delete_skips_undo_contents_exceeding_the_size_limit() {
    assert_eq!(apply_delete(&[FILE], FileEditOptions { max_size: Some(2.0), ..FileEditOptions::default() }), ["delete /workspace/entry-0"]);
}

/// workingCopyFileService.test.ts 'createFolder': the participant and the did event see the one folder.
#[test]
fn create_folder_runs_participants_and_reports_after() {
    let mut fs = Recorder::default();
    let folder = Resource::file("/path/folder");
    create_folder(&mut fs, std::slice::from_ref(&folder), UndoInfo::default()).unwrap();
    let files = vec![SourceTarget { source: None, target: folder }];
    assert_eq!(fs.events, [(FileOperation::Create, files.clone(), "will"), (FileOperation::Create, files, "did")]);
}

/// 'move - source identical to target': the events run though nothing moves.
#[test]
fn move_onto_itself_still_runs_the_events() {
    let mut fs = Recorder::default();
    fs.entries.insert("/path/file.txt".into(), (FILE, Vec::new()));
    let file = Resource::file("/path/file.txt");
    move_or_copy(&mut fs, &[MoveOrCopy { source: file.clone(), target: file, overwrite: true }], Mode::Move, UndoInfo::default()).unwrap();
    assert_eq!(fs.events.len(), 2);
    assert!(fs.log.is_empty());
}

/// workspace.test.ts 'applyEdit should fail when renaming deleted resource': delete A, then rename A to A.
#[test]
fn rename_of_a_deleted_file_onto_itself_fails() {
    let mut fs = Recorder::default();
    fs.entries.insert("/a".into(), (FILE, Vec::new()));
    let a = Resource::file("/a");
    let edits = vec![
        FileEdit::Delete { old: a.clone(), options: FileEditOptions::default(), undoes_create: false },
        FileEdit::Rename { old: a.clone(), new: a, options: FileEditOptions::default() },
    ];
    assert!(apply(&mut fs, edits, None).is_err());
}

/// Undo of a rename moves the file back without asking the participants (`isUndoing`), redo moves it again.
#[test]
fn rename_undo_and_redo_skip_participants() {
    let mut fs = Recorder::default();
    fs.entries.insert("/a".into(), (FILE, b"x".to_vec()));
    let (a, b) = (Resource::file("/a"), Resource::file("/b"));
    let mut undo = apply(&mut fs, vec![FileEdit::Rename { old: a.clone(), new: b.clone(), options: FileEditOptions::default() }], Some(1)).unwrap();
    undo.reverse(&mut fs).unwrap();
    undo.reverse(&mut fs).unwrap();
    assert_eq!(fs.log, ["move /a /b", "move /b /a", "move /a /b"]);
    let wills = fs.events.iter().filter(|(_, _, when)| *when == "will").count();
    assert_eq!(wills, 1);
    assert_eq!(fs.events.len(), 4);
}

/// `ignoreIfExists` without `overwrite` skips a create whose target exists; consecutive edits of a kind form one group.
#[test]
fn create_skips_existing_targets_and_groups_by_kind() {
    let mut fs = Recorder::default();
    fs.entries.insert("/a".into(), (FILE, b"keep".to_vec()));
    let skip = FileEditOptions { ignore_if_exists: Some(true), ..FileEditOptions::default() };
    let edits = vec![
        FileEdit::Create { new: Resource::file("/a"), options: skip, contents: Some(Vec::new()) },
        FileEdit::Create { new: Resource::file("/b"), options: FileEditOptions::default(), contents: Some(b"new".to_vec()) },
        FileEdit::Delete { old: Resource::file("/missing"), options: FileEditOptions { ignore_if_not_exists: Some(true), ..FileEditOptions::default() }, undoes_create: false },
    ];
    let undo = apply(&mut fs, edits, None).unwrap();
    assert_eq!(fs.log, ["create /b [110, 101, 119]"]);
    assert_eq!(undo.operations.len(), 2);
    assert_eq!(fs.entries["/a"].1, b"keep");
}

/// A missing file without `ignoreIfNotExists` fails the delete before anything runs.
#[test]
fn delete_of_a_missing_file_fails() {
    let mut fs = Recorder::default();
    let edit = FileEdit::Delete { old: Resource::file("/missing"), options: FileEditOptions::default(), undoes_create: false };
    assert_eq!(apply(&mut fs, vec![edit], None).unwrap_err().message, "file:///missing does not exist and can not be deleted");
    assert!(fs.events.is_empty());
}
