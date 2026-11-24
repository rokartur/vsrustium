//! The working copy file service (workbench/services/workingCopy/common/workingCopyFileService.ts) and bulk file edits
//! (workbench/contrib/bulkEdit/browser/bulkFileEdits.ts): file operations that run the file operation participants
//! first and report themselves after, and the workspace edits built from them with their undo.

use crate::{DIRECTORY, FILE, FileSystem, Mode, Resource, Result, TRASH};

/// `FileOperation` of platform/files/common/files.ts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileOperation {
    Create = 0,
    Delete = 1,
    Move = 2,
    Copy = 3,
}

/// `SourceTargetPair`.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceTarget {
    pub source: Option<Resource>,
    pub target: Resource,
}

/// `IFileOperationUndoRedoInfo`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UndoInfo {
    pub undo_redo_group_id: Option<u64>,
    pub is_undoing: bool,
}

/// What the working copy file service needs around the file service.
pub trait WorkingCopyFiles: FileSystem {
    /// `WorkingCopyFileOperationParticipant.participate`: the extension host's `onWill*` listeners and their edits.
    fn participate(&mut self, operation: FileOperation, files: &[SourceTarget], undo: UndoInfo);
    /// `onDidRunWorkingCopyFileOperation`.
    fn did_run(&mut self, operation: FileOperation, files: &[SourceTarget]);
    /// `files.enableTrash`.
    fn enable_trash(&mut self) -> bool;
    /// `ITextFileService.getEncodedReadable(resource)`: the bytes of a new empty file in its `files.encoding`.
    fn empty_file(&mut self, resource: &Resource) -> Vec<u8>;
}

/// `ICreateFileOperation`.
pub struct CreateFile {
    pub resource: Resource,
    pub contents: Vec<u8>,
    pub overwrite: bool,
}

/// `IMoveOperation` / `ICopyOperation`.
pub struct MoveOrCopy {
    pub source: Resource,
    pub target: Resource,
    pub overwrite: bool,
}

/// `IDeleteOperation`.
pub struct Delete {
    pub resource: Resource,
    pub recursive: bool,
    pub use_trash: bool,
}

/// `WorkingCopyFileService.create`.
pub fn create(fs: &mut impl WorkingCopyFiles, operations: &[CreateFile], undo: UndoInfo) -> Result<()> {
    if operations.is_empty() {
        return Ok(());
    }
    let mut validated = Ok(());
    for operation in operations {
        validated = validated.and(crate::validate_create_file(fs, &operation.resource, operation.overwrite));
    }
    validated?;
    let files: Vec<SourceTarget> = operations.iter().map(|operation| SourceTarget { source: None, target: operation.resource.clone() }).collect();
    run_participants(fs, FileOperation::Create, &files, undo);
    // `Promises.settled`: every create runs, the first failure is the result.
    let mut created = Ok(());
    for operation in operations {
        created = created.and(crate::create_file(fs, &operation.resource, &operation.contents, operation.overwrite));
    }
    created?;
    fs.did_run(FileOperation::Create, &files);
    Ok(())
}

/// `WorkingCopyFileService.createFolder`.
pub fn create_folder(fs: &mut impl WorkingCopyFiles, resources: &[Resource], undo: UndoInfo) -> Result<()> {
    if resources.is_empty() {
        return Ok(());
    }
    let files: Vec<SourceTarget> = resources.iter().map(|resource| SourceTarget { source: None, target: resource.clone() }).collect();
    run_participants(fs, FileOperation::Create, &files, undo);
    let mut created = Ok(());
    for resource in resources {
        created = created.and(crate::create_folder(fs, resource));
    }
    created?;
    fs.did_run(FileOperation::Create, &files);
    Ok(())
}

/// `WorkingCopyFileService.move` / `copy`.
pub fn move_or_copy(fs: &mut impl WorkingCopyFiles, operations: &[MoveOrCopy], mode: Mode, undo: UndoInfo) -> Result<()> {
    for operation in operations {
        crate::validate_move_or_copy(fs, &operation.source, &operation.target, mode, operation.overwrite)?;
    }
    let files: Vec<SourceTarget> = operations.iter().map(|operation| SourceTarget { source: Some(operation.source.clone()), target: operation.target.clone() }).collect();
    let kind = if mode == Mode::Move { FileOperation::Move } else { FileOperation::Copy };
    run_participants(fs, kind, &files, undo);
    // shortcut: dirty editors of the source and target are not reverted first; the working copies are milestone 8's.
    for operation in operations {
        crate::move_or_copy(fs, &operation.source, &operation.target, mode, operation.overwrite)?;
    }
    fs.did_run(kind, &files);
    Ok(())
}

/// `WorkingCopyFileService.delete`.
pub fn delete(fs: &mut impl WorkingCopyFiles, operations: &[Delete], undo: UndoInfo) -> Result<()> {
    for operation in operations {
        crate::validate_delete(fs, &operation.resource, operation.recursive, operation.use_trash)?;
    }
    let files: Vec<SourceTarget> = operations.iter().map(|operation| SourceTarget { source: None, target: operation.resource.clone() }).collect();
    run_participants(fs, FileOperation::Delete, &files, undo);
    for operation in operations {
        crate::delete(fs, &operation.resource, operation.recursive, operation.use_trash)?;
    }
    fs.did_run(FileOperation::Delete, &files);
    Ok(())
}

/// The file service participant (mainThreadFileSystemEventService.ts) skips operations that undo an earlier one.
fn run_participants(fs: &mut impl WorkingCopyFiles, operation: FileOperation, files: &[SourceTarget], undo: UndoInfo) {
    if !undo.is_undoing {
        fs.participate(operation, files, undo);
    }
}

/// `WorkspaceFileEditOptions`; unset and false differ where options are spread over defaults.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileEditOptions {
    pub overwrite: Option<bool>,
    pub ignore_if_not_exists: Option<bool>,
    pub ignore_if_exists: Option<bool>,
    pub recursive: Option<bool>,
    pub copy: Option<bool>,
    pub folder: Option<bool>,
    pub skip_trash_bin: Option<bool>,
    pub max_size: Option<f64>,
}

impl FileEditOptions {
    /// `{ recursive, folder, ...options }`: the given values unless the options set them.
    fn over(&self, recursive: bool, folder: bool) -> FileEditOptions {
        FileEditOptions { recursive: self.recursive.or(Some(recursive)), folder: self.folder.or(Some(folder)), ..self.clone() }
    }

    /// `overwrite === undefined && ignoreIfExists`: skip the edit when its target exists.
    fn skips_existing(&self) -> bool {
        self.overwrite.is_none() && self.ignore_if_exists == Some(true)
    }
}

/// `RenameEdit`, `CopyEdit`, `DeleteEdit` and `CreateEdit`.
#[derive(Debug, Clone, PartialEq)]
pub enum FileEdit {
    Rename { old: Resource, new: Resource, options: FileEditOptions },
    Copy { old: Resource, new: Resource, options: FileEditOptions },
    Delete { old: Resource, options: FileEditOptions, undoes_create: bool },
    Create { new: Resource, options: FileEditOptions, contents: Option<Vec<u8>> },
}

impl FileEdit {
    fn same_kind(&self, other: &FileEdit) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// `FileUndoRedoElement`: the edits that reverse each group of the applied edit, in order.
#[derive(Debug, Default, PartialEq)]
pub struct FileUndo {
    pub operations: Vec<Vec<FileEdit>>,
}

impl FileUndo {
    /// `undo` and `redo` (`_reverse`): performs each group and keeps what reverses it again.
    pub fn reverse(&mut self, fs: &mut impl WorkingCopyFiles) -> Result<()> {
        for operation in &mut self.operations {
            *operation = perform(fs, operation, UndoInfo { undo_redo_group_id: None, is_undoing: true })?;
        }
        Ok(())
    }

    /// `IWorkspaceUndoRedoElement.resources`.
    pub fn resources(&self) -> Vec<Resource> {
        let mut resources = Vec::new();
        for edit in self.operations.iter().flatten() {
            match edit {
                FileEdit::Rename { old, new, .. } | FileEdit::Copy { old, new, .. } => resources.extend([new.clone(), old.clone()]),
                FileEdit::Delete { old, .. } => resources.push(old.clone()),
                FileEdit::Create { new, .. } => resources.push(new.clone()),
            }
        }
        resources
    }
}

/// `BulkFileEdits.apply`: consecutive edits of one kind run as one operation; the result undoes them all.
pub fn apply(fs: &mut impl WorkingCopyFiles, edits: Vec<FileEdit>, undo_redo_group_id: Option<u64>) -> Result<FileUndo> {
    let mut groups: Vec<Vec<FileEdit>> = Vec::new();
    for edit in edits {
        match groups.last_mut() {
            Some(group) if group[0].same_kind(&edit) => group.push(edit),
            _ => groups.push(vec![edit]),
        }
    }
    let mut undo = FileUndo::default();
    for group in groups {
        undo.operations.push(perform(fs, &group, UndoInfo { undo_redo_group_id, is_undoing: false })?);
    }
    Ok(undo)
}

/// `RenameOperation`, `CopyOperation`, `DeleteOperation` or `CreateOperation.perform` for a group of one kind: the
/// edits that reverse it, empty for a `Noop`.
fn perform(fs: &mut impl WorkingCopyFiles, group: &[FileEdit], undo: UndoInfo) -> Result<Vec<FileEdit>> {
    match group.first() {
        Some(FileEdit::Rename { .. }) => rename(fs, group, undo),
        Some(FileEdit::Copy { .. }) => copy(fs, group, undo),
        Some(FileEdit::Delete { .. }) => delete_edits(fs, group, undo),
        Some(FileEdit::Create { .. }) => create_edits(fs, group, undo),
        None => Ok(Vec::new()),
    }
}

fn rename(fs: &mut impl WorkingCopyFiles, group: &[FileEdit], undo: UndoInfo) -> Result<Vec<FileEdit>> {
    let mut moves = Vec::new();
    let mut undoes = Vec::new();
    for edit in group {
        let FileEdit::Rename { old, new, options } = edit else { continue };
        if options.skips_existing() && crate::exists(fs, new)? {
            continue;
        }
        moves.push(MoveOrCopy { source: old.clone(), target: new.clone(), overwrite: options.overwrite == Some(true) });
        undoes.push(FileEdit::Rename { old: new.clone(), new: old.clone(), options: options.clone() });
    }
    if moves.is_empty() {
        return Ok(Vec::new());
    }
    move_or_copy(fs, &moves, Mode::Move, undo)?;
    Ok(undoes)
}

fn copy(fs: &mut impl WorkingCopyFiles, group: &[FileEdit], undo: UndoInfo) -> Result<Vec<FileEdit>> {
    let mut copies = Vec::new();
    for edit in group {
        let FileEdit::Copy { old, new, options } = edit else { continue };
        if options.skips_existing() && crate::exists(fs, new)? {
            continue;
        }
        copies.push(MoveOrCopy { source: old.clone(), target: new.clone(), overwrite: options.overwrite == Some(true) });
    }
    if copies.is_empty() {
        return Ok(Vec::new());
    }
    move_or_copy(fs, &copies, Mode::Copy, undo)?;
    // Upstream pairs the copied stats with the edits by index, skipped edits included.
    let mut undoes = Vec::new();
    for (index, copied) in copies.iter().enumerate() {
        let FileEdit::Copy { options, .. } = &group[index] else { continue };
        let is_directory = crate::resolve(fs, &copied.target)?.file_type & DIRECTORY != 0;
        let folder = options.folder == Some(true) || is_directory;
        undoes.push(FileEdit::Delete { old: copied.target.clone(), options: options.over(true, folder), undoes_create: false });
    }
    Ok(undoes)
}

fn create_edits(fs: &mut impl WorkingCopyFiles, group: &[FileEdit], undo: UndoInfo) -> Result<Vec<FileEdit>> {
    let mut folders = Vec::new();
    let mut files = Vec::new();
    let mut undoes = Vec::new();
    for edit in group {
        let FileEdit::Create { new, options, contents } = edit else { continue };
        // An untitled resource is left to a later text edit.
        if new.scheme == "untitled" {
            continue;
        }
        if options.skips_existing() && crate::exists(fs, new)? {
            continue;
        }
        let folder = options.folder == Some(true);
        if folder {
            folders.push(new.clone());
        } else {
            let bytes = contents.clone().unwrap_or_else(|| fs.empty_file(new));
            files.push(CreateFile { resource: new.clone(), contents: bytes, overwrite: options.overwrite == Some(true) });
        }
        undoes.push(FileEdit::Delete { old: new.clone(), options: options.clone(), undoes_create: !folder && contents.is_none() });
    }
    if folders.is_empty() && files.is_empty() {
        return Ok(Vec::new());
    }
    create_folder(fs, &folders, undo)?;
    create(fs, &files, undo)?;
    Ok(undoes)
}

fn delete_edits(fs: &mut impl WorkingCopyFiles, group: &[FileEdit], undo: UndoInfo) -> Result<Vec<FileEdit>> {
    let mut deletes = Vec::new();
    let mut undoes = Vec::new();
    for edit in group {
        let FileEdit::Delete { old, options, undoes_create } = edit else { continue };
        let Ok(stat) = crate::resolve(fs, old) else {
            if options.ignore_if_not_exists != Some(true) {
                return Err(crate::plain_error(format!("{} does not exist and can not be deleted", uri_string(old))));
            }
            continue;
        };
        let has_trash = fs.capabilities(&old.scheme).is_some_and(|capabilities| capabilities & TRASH != 0);
        let use_trash = options.skip_trash_bin != Some(true) && has_trash && fs.enable_trash();
        deletes.push(Delete { resource: old.clone(), recursive: options.recursive == Some(true), use_trash });
        if stat.file_type & (FILE | DIRECTORY) == 0 {
            continue;
        }
        // A file too large is not restored by undo.
        let mut contents = None;
        let mut exceeds_max_size = false;
        if !undoes_create && options.folder != Some(true) {
            exceeds_max_size = options.max_size.is_some_and(|max_size| stat.size as f64 > max_size);
            if !exceeds_max_size {
                match crate::read_file(fs, old) {
                    Ok(bytes) => contents = Some(bytes),
                    Err(error) => eprintln!("[bulk edit] {error}"),
                }
            }
        }
        if !exceeds_max_size {
            undoes.push(FileEdit::Create { new: old.clone(), options: options.clone(), contents });
        }
    }
    if deletes.is_empty() {
        return Ok(Vec::new());
    }
    delete(fs, &deletes, undo)?;
    Ok(undoes)
}

/// `URI.toString()` without its percent-encoding, for the error of a missing file.
fn uri_string(resource: &Resource) -> String {
    format!("{}://{}{}", resource.scheme, resource.authority, resource.path)
}

#[cfg(test)]
mod tests;
