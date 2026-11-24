//! platform/files `FileService` over file system providers: the disk (`disk::Disk`, DiskFileSystemProvider) and
//! whatever the caller adds through `FileSystem` (extension providers). Errors keep the JavaScript error names of each
//! layer; `handle_error` renames them as `MainThreadFileSystem._handleError` does before they reach the host.

pub mod disk;
pub mod glob;
pub mod watcher;

/// `FileType` of platform/files/common/files.ts.
pub const FILE: u32 = 1;
pub const DIRECTORY: u32 = 2;
pub const SYMBOLIC_LINK: u32 = 64;

/// `FilePermission`.
pub const READONLY: u32 = 1;
pub const LOCKED: u32 = 2;
pub const EXECUTABLE: u32 = 4;

/// `FileSystemProviderCapabilities`.
pub const FILE_FOLDER_COPY: u32 = 1 << 3;
pub const PATH_CASE_SENSITIVE: u32 = 1 << 10;
pub const PROVIDER_READONLY: u32 = 1 << 11;
pub const TRASH: u32 = 1 << 12;
pub const FILE_ATOMIC_DELETE: u32 = 1 << 16;

/// `FileSystemProviderErrorCode` values.
pub const FILE_EXISTS: &str = "EntryExists";
pub const FILE_NOT_FOUND: &str = "EntryNotFound";
pub const FILE_NOT_A_DIRECTORY: &str = "EntryNotADirectory";
pub const FILE_IS_A_DIRECTORY: &str = "EntryIsADirectory";
pub const FILE_EXCEEDS_STORAGE_QUOTA: &str = "EntryExceedsStorageQuota";
pub const FILE_TOO_LARGE: &str = "EntryTooLarge";
pub const FILE_WRITE_LOCKED: &str = "EntryWriteLocked";
pub const NO_PERMISSIONS: &str = "NoPermissions";
pub const UNAVAILABLE: &str = "Unavailable";
pub const UNKNOWN: &str = "Unknown";
const PROVIDER_CODES: [&str; 10] =
    [FILE_EXISTS, FILE_NOT_FOUND, FILE_NOT_A_DIRECTORY, FILE_IS_A_DIRECTORY, FILE_EXCEEDS_STORAGE_QUOTA, FILE_TOO_LARGE, FILE_WRITE_LOCKED, NO_PERMISSIONS, UNAVAILABLE, UNKNOWN];

/// `FileOperationResult` values `MainThreadFileSystem._handleError` and the service tell apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Operation {
    FileNotFound,
    FileIsDirectory,
    FileNotDirectory,
    FileWriteLocked,
    PermissionDenied,
    MoveConflict,
    FileTooLarge,
    InvalidPath,
    Other,
}

/// A JavaScript error as it crosses the layers: `name` and `message`, and the result of a `FileOperationError`.
#[derive(Debug, Clone, PartialEq)]
pub struct FileError {
    pub name: String,
    pub message: String,
    pub operation: Option<Operation>,
}

pub type Result<T> = std::result::Result<T, FileError>;

/// `createFileSystemProviderError`: the name `markAsFileSystemProviderError` gives, `EntryNotFound (FileSystemError)`.
pub fn provider_error(code: &str, message: impl Into<String>) -> FileError {
    FileError { name: format!("{code} (FileSystemError)"), message: message.into(), operation: None }
}

fn operation_error(operation: Operation, message: impl Into<String>) -> FileError {
    FileError { name: "Error".into(), message: message.into(), operation: Some(operation) }
}

fn plain_error(message: impl Into<String>) -> FileError {
    FileError { name: "Error".into(), message: message.into(), operation: None }
}

impl std::fmt::Display for FileError {
    /// `Error.prototype.toString`.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

/// `toFileSystemProviderErrorCode`.
pub fn provider_code(error: &FileError) -> &'static str {
    let Some(code) = error.name.strip_suffix(" (FileSystemError)") else { return UNKNOWN };
    PROVIDER_CODES.into_iter().find(|known| *known == code).unwrap_or(UNKNOWN)
}

/// `toFileOperationResult`.
fn operation_result(error: &FileError) -> Operation {
    if let Some(operation) = error.operation {
        return operation;
    }
    match provider_code(error) {
        FILE_NOT_FOUND => Operation::FileNotFound,
        FILE_IS_A_DIRECTORY => Operation::FileIsDirectory,
        FILE_NOT_A_DIRECTORY => Operation::FileNotDirectory,
        FILE_WRITE_LOCKED => Operation::FileWriteLocked,
        NO_PERMISSIONS => Operation::PermissionDenied,
        FILE_EXISTS => Operation::MoveConflict,
        FILE_TOO_LARGE => Operation::FileTooLarge,
        _ => Operation::Other,
    }
}

/// `MainThreadFileSystem._handleError`: the name the extension host maps to a `FileSystemError` code.
pub fn handle_error(mut error: FileError) -> FileError {
    let name = match error.operation {
        Some(Operation::FileNotFound) => Some(FILE_NOT_FOUND),
        Some(Operation::FileIsDirectory) => Some(FILE_IS_A_DIRECTORY),
        Some(Operation::PermissionDenied) => Some(NO_PERMISSIONS),
        Some(Operation::MoveConflict) => Some(FILE_EXISTS),
        Some(_) => None,
        None => Some(provider_code(&error)).filter(|code| *code != UNKNOWN),
    };
    if let Some(name) = name {
        error.name = name.into();
    }
    error
}

/// The parts of a `URI` the file service looks at.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    pub scheme: String,
    pub authority: String,
    pub path: String,
}

impl Resource {
    pub fn file(path: impl Into<String>) -> Self {
        Resource { scheme: "file".into(), authority: String::new(), path: path.into() }
    }

    /// `resourceForError`: the file system path of a `file` resource, else `toString(true)`.
    pub fn for_error(&self) -> String {
        if self.scheme == "file" {
            return self.path.clone();
        }
        match self.authority.is_empty() {
            true => format!("{}:{}", self.scheme, self.path),
            false => format!("{}://{}{}", self.scheme, self.authority, self.path),
        }
    }

    /// `extUri.dirname`.
    pub fn dirname(&self) -> Resource {
        let trimmed = self.path.trim_end_matches('/');
        let parent = match trimmed.rfind('/') {
            Some(0) | None => "/",
            Some(index) => &trimmed[..index],
        };
        Resource { path: parent.into(), ..self.clone() }
    }

    pub fn basename(&self) -> &str {
        self.path.trim_end_matches('/').rsplit('/').next().unwrap_or_default()
    }

    pub fn join(&self, name: &str) -> Resource {
        Resource { path: format!("{}/{name}", self.path.trim_end_matches('/')), ..self.clone() }
    }

    fn key(&self, ignore_case: bool) -> String {
        let key = format!("{}://{}{}", self.scheme, self.authority, self.path.trim_end_matches('/'));
        if ignore_case { key.to_lowercase() } else { key }
    }

    /// `extUri.isEqual` / `extUriIgnorePathCase.isEqual`.
    pub fn is_equal(&self, other: &Resource, ignore_case: bool) -> bool {
        self.key(ignore_case) == other.key(ignore_case)
    }

    /// `isEqualOrParent(self, ancestor)`: `self` is `ancestor` or below it.
    pub fn is_equal_or_below(&self, ancestor: &Resource, ignore_case: bool) -> bool {
        let (path, ancestor) = (self.key(ignore_case), ancestor.key(ignore_case));
        path == ancestor || path.strip_prefix(&ancestor).is_some_and(|rest| rest.starts_with('/'))
    }
}

/// `IStat`; times in milliseconds since the epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub file_type: u32,
    pub ctime: u64,
    pub mtime: u64,
    pub size: u64,
    pub permissions: Option<u32>,
}

/// The `IFileSystemProvider` calls the service makes, by scheme. `capabilities` answers `None` for a scheme without a
/// provider; it is also where a provider gets activated (`FileService.activateProvider`).
pub trait FileSystem {
    fn capabilities(&mut self, scheme: &str) -> Option<u32>;
    fn stat(&mut self, resource: &Resource) -> Result<Stat>;
    fn read_dir(&mut self, resource: &Resource) -> Result<Vec<(String, u32)>>;
    fn read_file(&mut self, resource: &Resource) -> Result<Vec<u8>>;
    fn write_file(&mut self, resource: &Resource, content: &[u8], create: bool, overwrite: bool) -> Result<()>;
    fn mkdir(&mut self, resource: &Resource) -> Result<()>;
    fn delete(&mut self, resource: &Resource, recursive: bool, use_trash: bool) -> Result<()>;
    fn rename(&mut self, from: &Resource, to: &Resource, overwrite: bool) -> Result<()>;
    fn copy(&mut self, from: &Resource, to: &Resource, overwrite: bool) -> Result<()>;
}

/// `FileService.withProvider`: the provider's capabilities.
fn with_provider(fs: &mut impl FileSystem, resource: &Resource) -> Result<u32> {
    if resource.scheme == "file" && !resource.path.starts_with('/') {
        return Err(operation_error(Operation::InvalidPath, format!("Unable to resolve filesystem provider with relative file path '{}'", resource.for_error())));
    }
    fs.capabilities(&resource.scheme).ok_or_else(|| FileError {
        name: "CodeExpectedError".into(),
        message: format!("ENOPRO: No file system provider found for resource '{}'", resource.for_error()),
        operation: None,
    })
}

/// `FileService.throwIfFileSystemIsReadonly`.
fn with_write_provider(fs: &mut impl FileSystem, resource: &Resource) -> Result<u32> {
    let capabilities = with_provider(fs, resource)?;
    if capabilities & PROVIDER_READONLY != 0 {
        return Err(operation_error(Operation::PermissionDenied, format!("Unable to modify read-only file '{}'", resource.for_error())));
    }
    Ok(capabilities)
}

/// `FileService.throwIfFileIsReadonly`.
fn throw_if_file_is_readonly(resource: &Resource, stat: &Stat) -> Result<()> {
    if stat.permissions.unwrap_or(0) & READONLY != 0 {
        return Err(operation_error(Operation::PermissionDenied, format!("Unable to modify read-only file '{}'", resource.for_error())));
    }
    Ok(())
}

/// `MainThreadFileSystem.$stat` over `FileService.stat`: one of file or folder plus the link bit, and `Readonly`
/// only when the file or its provider is read-only.
pub fn stat(fs: &mut impl FileSystem, resource: &Resource) -> Result<Stat> {
    let capabilities = with_provider(fs, resource)?;
    let stat = fs.stat(resource)?;
    let readonly = stat.permissions.unwrap_or(0) & READONLY != 0 || capabilities & PROVIDER_READONLY != 0;
    Ok(Stat { file_type: as_file_type(stat.file_type), permissions: readonly.then_some(READONLY), ..stat })
}

/// `MainThreadFileSystem._asFileType`.
fn as_file_type(file_type: u32) -> u32 {
    let kind = match (file_type & FILE != 0, file_type & DIRECTORY != 0) {
        (true, _) => FILE,
        (false, true) => DIRECTORY,
        (false, false) => 0,
    };
    kind | (file_type & SYMBOLIC_LINK)
}

/// `FileService.resolve` without children: a missing file becomes `FILE_NOT_FOUND`.
fn resolve(fs: &mut impl FileSystem, resource: &Resource) -> Result<Stat> {
    with_provider(fs, resource)?;
    fs.stat(resource).map_err(|error| match provider_code(&error) {
        FILE_NOT_FOUND => operation_error(Operation::FileNotFound, format!("Unable to resolve nonexistent file '{}'", resource.for_error())),
        _ => error,
    })
}

/// `MainThreadFileSystem.$readdir`: `resolve` stats every child and leaves out those that fail.
pub fn read_dir(fs: &mut impl FileSystem, resource: &Resource) -> Result<Vec<(String, u32)>> {
    let folder = resolve(fs, resource)?;
    if folder.file_type & DIRECTORY == 0 {
        return Err(FileError { name: FILE_NOT_A_DIRECTORY.into(), message: resource.basename().into(), operation: None });
    }
    let mut children = Vec::new();
    for (name, _) in fs.read_dir(resource)? {
        let Ok(child) = fs.stat(&resource.join(&name)) else { continue };
        children.push((name, as_file_type(child.file_type)));
    }
    Ok(children)
}

/// `FileService.readFile`: every failure becomes `Unable to read file '…' (<error>)` (`restoreReadError`).
pub fn read_file(fs: &mut impl FileSystem, resource: &Resource) -> Result<Vec<u8>> {
    let read = (|| {
        if resolve(fs, resource)?.file_type & DIRECTORY != 0 {
            return Err(operation_error(Operation::FileIsDirectory, format!("Unable to read file '{}' that is actually a directory", resource.for_error())));
        }
        fs.read_file(resource)
    })();
    read.map_err(|error| operation_error(operation_result(&error), format!("Unable to read file '{}' ({error})", resource.for_error())))
}

/// `FileService.writeFile`: refuses a folder or a read-only file, creates the missing parent folders, then writes.
pub fn write_file(fs: &mut impl FileSystem, resource: &Resource, content: &[u8]) -> Result<()> {
    with_write_provider(fs, resource)?;
    let write = (|| {
        let existing = fs.stat(resource).ok();
        if let Some(stat) = &existing {
            if stat.file_type & DIRECTORY != 0 {
                return Err(operation_error(Operation::FileIsDirectory, format!("Unable to write file '{}' that is actually a directory", resource.for_error())));
            }
            throw_if_file_is_readonly(resource, stat)?;
        }
        if existing.is_none() {
            mkdirp(fs, &resource.dirname())?;
        }
        fs.write_file(resource, content, true, true)
    })();
    write.map_err(|error| operation_error(operation_result(&error), format!("Unable to write file '{}' ({error})", resource.for_error())))
}

/// `FileService.createFolder`.
pub fn create_folder(fs: &mut impl FileSystem, resource: &Resource) -> Result<()> {
    with_write_provider(fs, resource)?;
    mkdirp(fs, resource)
}

/// `FileService.mkdirp`: walks up to the first existing folder, then creates each missing one.
fn mkdirp(fs: &mut impl FileSystem, directory: &Resource) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = directory.clone();
    while !current.is_equal(&current.dirname(), false) {
        match fs.stat(&current) {
            Ok(found) if found.file_type & DIRECTORY == 0 => {
                return Err(plain_error(format!("Unable to create folder '{}' that already exists but is not a directory", current.for_error())));
            }
            Ok(_) => break,
            Err(error) if provider_code(&error) == FILE_NOT_FOUND => {
                missing.push(current.basename().to_owned());
                current = current.dirname();
            }
            Err(error) => return Err(error),
        }
    }
    for name in missing.iter().rev() {
        current = current.join(name);
        match fs.mkdir(&current) {
            Err(error) if provider_code(&error) != FILE_EXISTS => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// `FileService.del` with `doValidateDelete`.
pub fn delete(fs: &mut impl FileSystem, resource: &Resource, recursive: bool, use_trash: bool) -> Result<()> {
    let capabilities = with_write_provider(fs, resource)?;
    if use_trash && capabilities & TRASH == 0 {
        return Err(plain_error(format!("Unable to delete file '{}' via trash because provider does not support it.", resource.for_error())));
    }
    let Ok(stat) = fs.stat(resource) else {
        return Err(operation_error(Operation::FileNotFound, format!("Unable to delete nonexistent file '{}'", resource.for_error())));
    };
    throw_if_file_is_readonly(resource, &stat)?;
    if !recursive && resolve(fs, resource)?.file_type & DIRECTORY != 0 && !fs.read_dir(resource)?.is_empty() {
        return Err(plain_error(format!("Unable to delete non-empty folder '{}'.", resource.for_error())));
    }
    fs.delete(resource, recursive, use_trash)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Move,
    Copy,
}

/// `FileService.move` / `copy` (`doMoveCopy` with `doValidateMoveCopy`).
pub fn move_or_copy(fs: &mut impl FileSystem, source: &Resource, target: &Resource, mode: Mode, overwrite: bool) -> Result<()> {
    let source_capabilities = match mode {
        Mode::Move => with_write_provider(fs, source)?,
        Mode::Copy => with_provider(fs, source)?,
    };
    let target_capabilities = with_write_provider(fs, target)?;
    let same_provider = source.scheme == target.scheme;
    if source.is_equal(target, false) {
        return Ok(());
    }

    let ignore_case = target_capabilities & PATH_CASE_SENSITIVE == 0;
    let mut same_with_different_case = false;
    if same_provider {
        same_with_different_case = ignore_case && source.is_equal(target, true);
        if same_with_different_case && mode == Mode::Copy {
            return Err(plain_error(format!(
                "Unable to copy when source '{}' is same as target '{}' with different path case on a case insensitive file system",
                source.for_error(),
                target.for_error()
            )));
        }
        if !same_with_different_case && target.is_equal_or_below(source, ignore_case) {
            return Err(plain_error(format!("Unable to move/copy when source '{}' is parent of target '{}'.", source.for_error(), target.for_error())));
        }
    }
    let exists = fs.stat(target).is_ok();
    if exists && !same_with_different_case {
        if !overwrite {
            return Err(operation_error(
                Operation::MoveConflict,
                format!("Unable to move/copy '{}' because target '{}' already exists at destination.", source.for_error(), target.for_error()),
            ));
        }
        if same_provider && source.is_equal_or_below(target, ignore_case) {
            return Err(plain_error(format!(
                "Unable to move/copy '{}' into '{}' since a file would replace the folder it is contained in.",
                source.for_error(),
                target.for_error()
            )));
        }
        delete(fs, target, true, false)?;
    }
    mkdirp(fs, &target.dirname())?;

    match mode {
        Mode::Copy if same_provider && source_capabilities & FILE_FOLDER_COPY != 0 => fs.copy(source, target, overwrite)?,
        Mode::Copy => copy_across(fs, source, target)?,
        Mode::Move if same_provider => fs.rename(source, target, overwrite)?,
        Mode::Move => {
            move_or_copy(fs, source, target, Mode::Copy, overwrite)?;
            delete(fs, source, true, false)?;
        }
    }
    resolve(fs, target).map(|_| ())
}

/// `doCopyFolder` / `doCopyFile`: copies through `readFile` and `writeFile` when the provider has no `copy`.
fn copy_across(fs: &mut impl FileSystem, source: &Resource, target: &Resource) -> Result<()> {
    if resolve(fs, source)?.file_type & DIRECTORY == 0 {
        let content = fs.read_file(source)?;
        return fs.write_file(target, &content, true, true);
    }
    fs.mkdir(target)?;
    for (name, _) in fs.read_dir(source)? {
        copy_across(fs, &source.join(&name), &target.join(&name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::Disk;
    use std::collections::BTreeMap;

    fn scratch(name: &str) -> Resource {
        let dir = std::env::temp_dir().join(format!("files-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Resource::file(dir.to_string_lossy())
    }

    fn host_name(error: FileError) -> String {
        handle_error(error).name
    }

    #[test]
    fn writes_create_parents_and_folders_nest() {
        let dir = scratch("write");
        let fs = &mut Disk;
        write_file(fs, &dir.join("a/b/c.txt"), b"hi").unwrap();
        assert_eq!(read_file(fs, &dir.join("a/b/c.txt")).unwrap(), b"hi");
        create_folder(fs, &dir.join("x/y/z")).unwrap();
        create_folder(fs, &dir.join("x/y/z")).unwrap();
        assert_eq!(stat(fs, &dir.join("x/y/z")).unwrap().file_type, DIRECTORY);
        assert_eq!(host_name(write_file(fs, &dir.join("x"), b"").unwrap_err()), FILE_IS_A_DIRECTORY);
        assert!(create_folder(fs, &dir.join("a/b/c.txt")).unwrap_err().message.starts_with("Unable to create folder"));
        assert_eq!(host_name(create_folder(fs, &dir.join("a/b/c.txt/d")).unwrap_err()), FILE_NOT_A_DIRECTORY);
        std::fs::remove_dir_all(&dir.path).unwrap();
    }

    #[test]
    fn errors_carry_the_code_the_host_maps() {
        let dir = scratch("errors");
        let fs = &mut Disk;
        let missing = stat(fs, &dir.join("missing")).unwrap_err();
        assert_eq!(missing.message, format!("ENOENT: no such file or directory, lstat '{}/missing'", dir.path));
        assert_eq!(host_name(missing), FILE_NOT_FOUND);
        assert_eq!(host_name(read_dir(fs, &dir.join("missing")).unwrap_err()), FILE_NOT_FOUND);
        let unread = read_file(fs, &dir.join("missing")).unwrap_err();
        assert_eq!(unread.message, format!("Unable to read file '{0}/missing' (Error: Unable to resolve nonexistent file '{0}/missing')", dir.path));
        assert_eq!(host_name(unread), FILE_NOT_FOUND);
        assert_eq!(host_name(delete(fs, &dir.join("missing"), false, false).unwrap_err()), FILE_NOT_FOUND);
        assert_eq!(host_name(move_or_copy(fs, &dir.join("missing"), &dir.join("other"), Mode::Move, false).unwrap_err()), FILE_NOT_FOUND);
        write_file(fs, &dir.join("folder/file"), b"").unwrap();
        assert_eq!(delete(fs, &dir.join("folder"), false, false).unwrap_err().message, format!("Unable to delete non-empty folder '{}/folder'.", dir.path));
        let enopro = stat(fs, &Resource { scheme: "foo".into(), authority: String::new(), path: "/bar".into() }).unwrap_err();
        assert_eq!(enopro.message, "ENOPRO: No file system provider found for resource 'foo:/bar'");
        std::fs::remove_dir_all(&dir.path).unwrap();
    }

    #[test]
    fn moves_and_copies_like_the_file_service() {
        let dir = scratch("move");
        let fs = &mut Disk;
        write_file(fs, &dir.join("src/a.txt"), b"a").unwrap();
        write_file(fs, &dir.join("b.txt"), b"b").unwrap();
        assert_eq!(host_name(move_or_copy(fs, &dir.join("b.txt"), &dir.join("src/a.txt"), Mode::Copy, false).unwrap_err()), FILE_EXISTS);
        move_or_copy(fs, &dir.join("b.txt"), &dir.join("src/a.txt"), Mode::Copy, true).unwrap();
        assert_eq!(read_file(fs, &dir.join("src/a.txt")).unwrap(), b"b");
        move_or_copy(fs, &dir.join("src"), &dir.join("deep/copy"), Mode::Copy, false).unwrap();
        assert_eq!(read_file(fs, &dir.join("deep/copy/a.txt")).unwrap(), b"b");
        assert!(move_or_copy(fs, &dir.join("src"), &dir.join("src/inner"), Mode::Move, false).unwrap_err().message.contains("is parent of target"));
        move_or_copy(fs, &dir.join("src"), &dir.join("SRC"), Mode::Move, false).unwrap();
        assert_eq!(read_dir(fs, &dir).unwrap().into_iter().filter(|(name, _)| name == "SRC").count(), 1);
        delete(fs, &dir.join("deep"), true, false).unwrap();
        assert!(stat(fs, &dir.join("deep")).is_err());
        std::fs::remove_dir_all(&dir.path).unwrap();
    }

    /// A case sensitive `mem` provider without `copy` next to the disk, as the api tests' `TestFS`.
    #[derive(Default)]
    struct Memory {
        entries: BTreeMap<String, Option<Vec<u8>>>,
    }

    impl FileSystem for Memory {
        fn capabilities(&mut self, scheme: &str) -> Option<u32> {
            match scheme {
                "mem" => Some(2 | PATH_CASE_SENSITIVE),
                _ => Disk.capabilities(scheme),
            }
        }
        fn stat(&mut self, resource: &Resource) -> Result<Stat> {
            if resource.scheme == "file" {
                return Disk.stat(resource);
            }
            let file_type = match self.entries.get(&resource.path) {
                Some(Some(_)) => FILE,
                Some(None) => DIRECTORY,
                None if resource.path == "/" => DIRECTORY,
                None => return Err(provider_error(FILE_NOT_FOUND, resource.path.clone())),
            };
            Ok(Stat { file_type, ctime: 0, mtime: 0, size: 0, permissions: None })
        }
        fn read_dir(&mut self, resource: &Resource) -> Result<Vec<(String, u32)>> {
            if resource.scheme == "file" {
                return Disk.read_dir(resource);
            }
            let prefix = format!("{}/", resource.path.trim_end_matches('/'));
            let mut children = Vec::new();
            for (path, content) in &self.entries {
                let Some(name) = path.strip_prefix(&prefix).filter(|name| !name.contains('/')) else { continue };
                children.push((name.to_owned(), if content.is_some() { FILE } else { DIRECTORY }));
            }
            Ok(children)
        }
        fn read_file(&mut self, resource: &Resource) -> Result<Vec<u8>> {
            if resource.scheme == "file" {
                return Disk.read_file(resource);
            }
            self.entries.get(&resource.path).cloned().flatten().ok_or_else(|| provider_error(FILE_NOT_FOUND, resource.path.clone()))
        }
        fn write_file(&mut self, resource: &Resource, content: &[u8], create: bool, overwrite: bool) -> Result<()> {
            if resource.scheme == "file" {
                return Disk.write_file(resource, content, create, overwrite);
            }
            self.entries.insert(resource.path.clone(), Some(content.to_vec()));
            Ok(())
        }
        fn mkdir(&mut self, resource: &Resource) -> Result<()> {
            if resource.scheme == "file" {
                return Disk.mkdir(resource);
            }
            self.entries.insert(resource.path.clone(), None);
            Ok(())
        }
        fn delete(&mut self, resource: &Resource, recursive: bool, use_trash: bool) -> Result<()> {
            if resource.scheme == "file" {
                return Disk.delete(resource, recursive, use_trash);
            }
            self.entries.retain(|path, _| !Resource::file(path.clone()).is_equal_or_below(&Resource::file(resource.path.clone()), false));
            Ok(())
        }
        fn rename(&mut self, from: &Resource, to: &Resource, _overwrite: bool) -> Result<()> {
            let content = self.entries.remove(&from.path).ok_or_else(|| provider_error(FILE_NOT_FOUND, from.path.clone()))?;
            self.entries.insert(to.path.clone(), content);
            Ok(())
        }
        fn copy(&mut self, _from: &Resource, _to: &Resource, _overwrite: bool) -> Result<()> {
            unreachable!("Memory has no FileFolderCopy capability")
        }
    }

    #[test]
    fn crosses_providers_through_read_and_write() {
        let dir = scratch("across");
        let fs = &mut Memory::default();
        let mem = |path: &str| Resource { scheme: "mem".into(), authority: String::new(), path: path.into() };
        write_file(fs, &mem("/folder/a.txt"), b"a").unwrap();
        assert_eq!(read_dir(fs, &mem("/")).unwrap(), vec![("folder".to_owned(), DIRECTORY)]);
        move_or_copy(fs, &mem("/folder"), &dir.join("copied"), Mode::Copy, false).unwrap();
        assert_eq!(read_file(fs, &dir.join("copied/a.txt")).unwrap(), b"a");
        move_or_copy(fs, &dir.join("copied"), &mem("/moved"), Mode::Move, false).unwrap();
        assert_eq!(read_file(fs, &mem("/moved/a.txt")).unwrap(), b"a");
        assert!(stat(fs, &dir.join("copied")).is_err());
        // workspace.fs.test "#132981": renaming a missing file reports the provider's FileNotFound.
        let missing = move_or_copy(fs, &mem("/moved/a.txt/b"), &mem("/c"), Mode::Move, false).unwrap_err();
        assert_eq!(host_name(missing), FILE_NOT_FOUND);
        std::fs::remove_dir_all(&dir.path).unwrap();
    }
}
