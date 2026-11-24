//! The `file` scheme of the file service: platform/files `FileService` over `DiskFileSystemProvider`, with the
//! errors named as `MainThreadFileSystem._handleError` names them for the extension host.

pub mod watcher;

use std::io::ErrorKind;
use std::path::Path;
use std::time::SystemTime;

/// `FileType` of platform/files/common/files.ts.
pub const FILE: u32 = 1;
pub const DIRECTORY: u32 = 2;
pub const SYMBOLIC_LINK: u32 = 64;

/// `FilePermission`.
const LOCKED: u32 = 2;
const EXECUTABLE: u32 = 8;

/// `FileSystemProviderErrorCode` values.
pub const FILE_EXISTS: &str = "EntryExists";
pub const FILE_NOT_FOUND: &str = "EntryNotFound";
pub const FILE_NOT_A_DIRECTORY: &str = "EntryNotADirectory";
pub const FILE_IS_A_DIRECTORY: &str = "EntryIsADirectory";
pub const NO_PERMISSIONS: &str = "NoPermissions";
pub const UNKNOWN: &str = "Unknown";
/// A plain `Error` without a provider code.
pub const ERROR: &str = "Error";

#[derive(Debug, Clone, PartialEq)]
pub struct FileError {
    pub name: &'static str,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, FileError>;

fn error(name: &'static str, message: impl Into<String>) -> FileError {
    FileError { name, message: message.into() }
}

/// `toFileSystemProviderError` of an errno exception, with Node's message (`ENOENT: no such file or directory, stat '/a'`).
fn io_error(err: std::io::Error, syscall: &str, path: &Path) -> FileError {
    let (code, name, text) = match err.raw_os_error() {
        Some(2) => ("ENOENT", FILE_NOT_FOUND, "no such file or directory"),
        Some(21) => ("EISDIR", FILE_IS_A_DIRECTORY, "illegal operation on a directory"),
        Some(20) => ("ENOTDIR", FILE_NOT_A_DIRECTORY, "not a directory"),
        Some(17) => ("EEXIST", FILE_EXISTS, "file already exists"),
        Some(1) => ("EPERM", NO_PERMISSIONS, "operation not permitted"),
        Some(13) => ("EACCES", NO_PERMISSIONS, "permission denied"),
        Some(66) => ("ENOTEMPTY", UNKNOWN, "directory not empty"),
        _ => return error(UNKNOWN, format!("{err}, {syscall} '{}'", path.display())),
    };
    error(name, format!("{code}: {text}, {syscall} '{}'", path.display()))
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

/// `DiskFileSystemProvider.stat`: `SymlinkSupport.stat` follows a link and marks it; `ctime` is the birth time.
pub fn stat(path: &Path) -> Result<Stat> {
    let link = std::fs::symlink_metadata(path).map_err(|err| io_error(err, "lstat", path))?;
    let is_link = link.file_type().is_symlink();
    let (metadata, dangling) = match is_link {
        true => match std::fs::metadata(path) {
            Ok(target) => (target, false),
            Err(err) if err.kind() == ErrorKind::NotFound => (link, true),
            Err(err) => return Err(io_error(err, "stat", path)),
        },
        false => (link, false),
    };
    let mut file_type = match (dangling, metadata.is_dir(), metadata.is_file()) {
        (true, _, _) => 0,
        (false, true, _) => DIRECTORY,
        (false, false, true) => FILE,
        _ => 0,
    };
    if is_link {
        file_type |= SYMBOLIC_LINK;
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode();
    let mut permissions = None;
    if mode & 0o200 == 0 {
        permissions = Some(LOCKED);
    }
    if mode & 0o111 != 0 {
        permissions = Some(permissions.unwrap_or(0) | EXECUTABLE);
    }
    let millis = |time: std::io::Result<SystemTime>| time.ok().and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |since| since.as_millis() as u64);
    Ok(Stat { file_type, ctime: millis(metadata.created()), mtime: millis(metadata.modified()), size: metadata.len(), permissions })
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// `DiskFileSystemProvider.readdir`: links report their target's type; entries that fail to stat are left out.
pub fn read_dir(path: &Path) -> Result<Vec<(String, u32)>> {
    let entries = std::fs::read_dir(path).map_err(|err| io_error(err, "scandir", path))?;
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let Ok(child) = stat(&entry.path()) else { continue };
        children.push((entry.file_name().to_string_lossy().into_owned(), child.file_type));
    }
    Ok(children)
}

pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(error(FILE_IS_A_DIRECTORY, format!("Unable to read file '{}' that is actually a directory", path.display())));
    }
    std::fs::read(path).map_err(|err| io_error(err, "open", path))
}

/// `FileService.writeFile`: refuses a directory, creates the missing parent folders, then writes.
pub fn write_file(path: &Path, content: &[u8]) -> Result<()> {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(error(FILE_IS_A_DIRECTORY, format!("Unable to write file '{}' that is actually a directory", path.display())));
    }
    if let Some(parent) = path.parent() {
        mkdirp(parent)?;
    }
    std::fs::write(path, content).map_err(|err| io_error(err, "open", path))
}

/// `FileService.createFolder`.
pub fn create_folder(path: &Path) -> Result<()> {
    mkdirp(path)
}

/// `FileService.mkdirp`: walks up to the first existing folder, then creates each missing one.
fn mkdirp(directory: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = directory;
    loop {
        match stat(current) {
            Ok(found) if found.file_type & DIRECTORY == 0 => {
                return Err(error(ERROR, format!("Unable to create folder '{}' that already exists but is not a directory", current.display())));
            }
            Ok(_) => break,
            Err(err) if err.name == FILE_NOT_FOUND => missing.push(current),
            Err(err) => return Err(err),
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    for folder in missing.into_iter().rev() {
        match std::fs::create_dir(folder) {
            Err(err) if err.kind() != ErrorKind::AlreadyExists => return Err(io_error(err, "mkdir", folder)),
            _ => {}
        }
    }
    Ok(())
}

/// `FileService.del` with `doValidateDelete`; `use_trash` moves the item to the Trash as Electron's `shell.trashItem`.
pub fn delete(path: &Path, recursive: bool, use_trash: bool) -> Result<()> {
    if !exists(path) {
        return Err(error(FILE_NOT_FOUND, format!("Unable to delete nonexistent file '{}'", path.display())));
    }
    let is_dir = std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
    if !recursive && is_dir && std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(error(ERROR, format!("Unable to delete non-empty folder '{}'.", path.display())));
    }
    if use_trash {
        let mut context = trash::TrashContext::default();
        trash::macos::TrashContextExtMacos::set_delete_method(&mut context, trash::macos::DeleteMethod::NsFileManager);
        return context.delete(path).map_err(|err| error(UNKNOWN, err.to_string()));
    }
    let removed = match (is_dir, recursive) {
        (true, true) => std::fs::remove_dir_all(path),
        (true, false) => std::fs::remove_dir(path),
        (false, _) => std::fs::remove_file(path),
    };
    removed.map_err(|err| io_error(err, if is_dir { "rmdir" } else { "unlink" }, path))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Move,
    Copy,
}

/// `FileService.move` / `copy` (`doMoveCopy`, `doValidateMoveCopy`) over the disk provider's `rename` / `copy`.
/// The disk is case insensitive on macOS, so paths compare ignoring case as upstream's `extUriIgnorePathCase`.
pub fn move_or_copy(source: &Path, target: &Path, mode: Mode, overwrite: bool) -> Result<()> {
    if source == target {
        return Ok(());
    }
    let same_ignoring_case = source.to_string_lossy().to_lowercase() == target.to_string_lossy().to_lowercase();
    if same_ignoring_case && mode == Mode::Copy {
        return Err(error(
            ERROR,
            format!(
                "Unable to copy when source '{}' is same as target '{}' with different path case on a case insensitive file system",
                source.display(),
                target.display()
            ),
        ));
    }
    if !same_ignoring_case && is_equal_or_parent(target, source) {
        return Err(error(ERROR, format!("Unable to move/copy when source '{}' is parent of target '{}'.", source.display(), target.display())));
    }
    let target_exists = exists(target);
    if target_exists && !same_ignoring_case {
        if !overwrite {
            return Err(error(
                FILE_EXISTS,
                format!("Unable to move/copy '{}' because target '{}' already exists at destination.", source.display(), target.display()),
            ));
        }
        if is_equal_or_parent(source, target) {
            return Err(error(ERROR, format!("Unable to move/copy '{}' into '{}' since a file would replace the folder it is contained in.", source.display(), target.display())));
        }
        delete(target, true, false)?;
    }
    if let Some(parent) = target.parent() {
        mkdirp(parent)?;
    }
    // DiskFileSystemProvider.validateMoveCopy
    let Ok(source_stat) = stat(source) else { return Err(error(FILE_NOT_FOUND, "File to move/copy does not exist")) };
    match mode {
        Mode::Move => std::fs::rename(source, target).map_err(|err| io_error(err, "rename", source)),
        Mode::Copy if same_ignoring_case => Err(error(FILE_EXISTS, "File cannot be copied to same path with different path case")),
        Mode::Copy if source_stat.file_type & DIRECTORY != 0 && source_stat.file_type & SYMBOLIC_LINK == 0 => copy_folder(source, target),
        Mode::Copy => copy_entry(source, target),
    }
}

/// pfs.ts `copy` with `preserveSymlinks`: links are recreated, not followed.
fn copy_entry(source: &Path, target: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source).map_err(|err| io_error(err, "lstat", source))?;
    if metadata.file_type().is_symlink() {
        let link = std::fs::read_link(source).map_err(|err| io_error(err, "readlink", source))?;
        return std::os::unix::fs::symlink(link, target).map_err(|err| io_error(err, "symlink", target));
    }
    if metadata.is_dir() {
        return copy_folder(source, target);
    }
    std::fs::copy(source, target).map(|_| ()).map_err(|err| io_error(err, "copyfile", source))
}

fn copy_folder(source: &Path, target: &Path) -> Result<()> {
    let permissions = std::fs::metadata(source).map_err(|err| io_error(err, "stat", source))?.permissions();
    match std::fs::create_dir(target) {
        Err(err) if err.kind() != ErrorKind::AlreadyExists => return Err(io_error(err, "mkdir", target)),
        _ => {}
    }
    std::fs::set_permissions(target, permissions).map_err(|err| io_error(err, "chmod", target))?;
    for entry in std::fs::read_dir(source).map_err(|err| io_error(err, "scandir", source))?.flatten() {
        copy_entry(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

/// `extUriIgnorePathCase.isEqualOrParent`.
pub fn is_equal_or_parent(path: &Path, candidate: &Path) -> bool {
    let path = path.to_string_lossy().to_lowercase();
    let candidate = candidate.to_string_lossy().to_lowercase();
    path == candidate || path.strip_prefix(&candidate).is_some_and(|rest| rest.starts_with('/') || candidate.ends_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("files-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_create_parents_and_folders_nest() {
        let dir = scratch("write");
        write_file(&dir.join("a/b/c.txt"), b"hi").unwrap();
        assert_eq!(read_file(&dir.join("a/b/c.txt")).unwrap(), b"hi");
        create_folder(&dir.join("x/y/z")).unwrap();
        create_folder(&dir.join("x/y/z")).unwrap();
        assert_eq!(stat(&dir.join("x/y/z")).unwrap().file_type, DIRECTORY);
        assert_eq!(write_file(&dir.join("x"), b"").unwrap_err().name, FILE_IS_A_DIRECTORY);
        assert!(create_folder(&dir.join("a/b/c.txt")).unwrap_err().message.starts_with("Unable to create folder"));
        assert_eq!(create_folder(&dir.join("a/b/c.txt/d")).unwrap_err().name, FILE_NOT_A_DIRECTORY);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn errors_carry_the_code_the_host_maps() {
        let dir = scratch("errors");
        let missing = stat(&dir.join("missing")).unwrap_err();
        assert_eq!(missing.name, FILE_NOT_FOUND);
        assert_eq!(missing.message, format!("ENOENT: no such file or directory, lstat '{}'", dir.join("missing").display()));
        assert_eq!(read_dir(&dir.join("missing")).unwrap_err().name, FILE_NOT_FOUND);
        assert_eq!(delete(&dir.join("missing"), false, false).unwrap_err().name, FILE_NOT_FOUND);
        assert_eq!(move_or_copy(&dir.join("missing"), &dir.join("other"), Mode::Move, false).unwrap_err().name, FILE_NOT_FOUND);
        write_file(&dir.join("folder/file"), b"").unwrap();
        assert_eq!(delete(&dir.join("folder"), false, false).unwrap_err().message, format!("Unable to delete non-empty folder '{}'.", dir.join("folder").display()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn moves_and_copies_like_the_file_service() {
        let dir = scratch("move");
        write_file(&dir.join("src/a.txt"), b"a").unwrap();
        write_file(&dir.join("b.txt"), b"b").unwrap();
        assert_eq!(move_or_copy(&dir.join("b.txt"), &dir.join("src/a.txt"), Mode::Copy, false).unwrap_err().name, FILE_EXISTS);
        move_or_copy(&dir.join("b.txt"), &dir.join("src/a.txt"), Mode::Copy, true).unwrap();
        assert_eq!(read_file(&dir.join("src/a.txt")).unwrap(), b"b");
        move_or_copy(&dir.join("src"), &dir.join("deep/copy"), Mode::Copy, false).unwrap();
        assert_eq!(read_file(&dir.join("deep/copy/a.txt")).unwrap(), b"b");
        assert!(move_or_copy(&dir.join("src"), &dir.join("src/inner"), Mode::Move, false).unwrap_err().message.contains("is parent of target"));
        move_or_copy(&dir.join("src"), &dir.join("SRC"), Mode::Move, false).unwrap();
        assert_eq!(read_dir(&dir).unwrap().into_iter().filter(|(name, _)| name == "SRC").count(), 1);
        delete(&dir.join("deep"), true, false).unwrap();
        assert!(!exists(&dir.join("deep")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
