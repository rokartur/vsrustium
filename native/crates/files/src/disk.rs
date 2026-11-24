//! `DiskFileSystemProvider` (platform/files/node/diskFileSystemProvider.ts) with the Trash of
//! `DiskFileSystemProviderClient` / Electron's `shell.trashItem`.

use std::io::ErrorKind;
use std::path::Path;
use std::time::SystemTime;

use crate::{DIRECTORY, EXECUTABLE, FILE, FileSystem, LOCKED, Resource, Result, SYMBOLIC_LINK, Stat};
use crate::{FILE_EXISTS, FILE_IS_A_DIRECTORY, FILE_NOT_A_DIRECTORY, FILE_NOT_FOUND, NO_PERMISSIONS, UNAVAILABLE, UNKNOWN, provider_error};

/// diskFileSystemProviderClient.ts on macOS: FileReadWrite, FileOpenReadWriteClose, FileReadStream, FileFolderCopy,
/// FileWriteUnlock, FileAtomicRead/Write/Delete, FileClone, FileRealpath, FileAppend, Trash; no PathCaseSensitive.
pub const CAPABILITIES: u32 = 2 | 4 | 16 | 8 | (1 << 13) | (1 << 14) | (1 << 15) | (1 << 16) | (1 << 17) | (1 << 18) | (1 << 19) | (1 << 12);

/// The `file` scheme only.
pub struct Disk;

/// `toFileSystemProviderError` of an errno exception, with Node's message (`ENOENT: no such file or directory, stat '/a'`).
fn io_error(err: std::io::Error, syscall: &str, path: &Path) -> crate::FileError {
    let (code, name, text) = match err.raw_os_error() {
        Some(2) => ("ENOENT", FILE_NOT_FOUND, "no such file or directory"),
        Some(21) => ("EISDIR", FILE_IS_A_DIRECTORY, "illegal operation on a directory"),
        Some(20) => ("ENOTDIR", FILE_NOT_A_DIRECTORY, "not a directory"),
        Some(17) => ("EEXIST", FILE_EXISTS, "file already exists"),
        Some(1) => ("EPERM", NO_PERMISSIONS, "operation not permitted"),
        Some(13) => ("EACCES", NO_PERMISSIONS, "permission denied"),
        Some(66) => ("ENOTEMPTY", UNKNOWN, "directory not empty"),
        _ => return provider_error(UNKNOWN, format!("{err}, {syscall} '{}'", path.display())),
    };
    provider_error(name, format!("{code}: {text}, {syscall} '{}'", path.display()))
}

fn path(resource: &Resource) -> &Path {
    Path::new(&resource.path)
}

/// `SymlinkSupport.stat`: follows a link and marks it; `ctime` is the birth time.
pub fn stat_path(path: &Path) -> Result<Stat> {
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

/// `validateMoveCopy`: the source must exist; an existing target needs `overwrite` and is deleted first unless both are files.
fn validate_move_copy(from: &Path, to: &Path, copy: bool, overwrite: bool) -> Result<Stat> {
    let Ok(from_stat) = stat_path(from) else { return Err(provider_error(FILE_NOT_FOUND, "File to move/copy does not exist")) };
    if from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase() {
        if copy {
            return Err(provider_error(FILE_EXISTS, "File cannot be copied to same path with different path case"));
        }
        return Ok(from_stat);
    }
    let Ok(to_stat) = stat_path(to) else { return Ok(from_stat) };
    if !overwrite {
        return Err(provider_error(FILE_EXISTS, "File at target already exists and thus will not be moved/copied to unless overwrite is specified"));
    }
    if from_stat.file_type & FILE == 0 || to_stat.file_type & FILE == 0 {
        Disk.delete(&Resource::file(to.to_string_lossy()), true, false)?;
    }
    Ok(from_stat)
}

/// pfs.ts `copy` with `preserveSymlinks`: links are recreated, not followed.
fn copy_entry(source: &Path, target: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source).map_err(|err| io_error(err, "lstat", source))?;
    if metadata.file_type().is_symlink() {
        let link = std::fs::read_link(source).map_err(|err| io_error(err, "readlink", source))?;
        return std::os::unix::fs::symlink(link, target).map_err(|err| io_error(err, "symlink", target));
    }
    if !metadata.is_dir() {
        return std::fs::copy(source, target).map(|_| ()).map_err(|err| io_error(err, "copyfile", source));
    }
    match std::fs::create_dir(target) {
        Err(err) if err.kind() != ErrorKind::AlreadyExists => return Err(io_error(err, "mkdir", target)),
        _ => {}
    }
    std::fs::set_permissions(target, metadata.permissions()).map_err(|err| io_error(err, "chmod", target))?;
    for entry in std::fs::read_dir(source).map_err(|err| io_error(err, "scandir", source))?.flatten() {
        copy_entry(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

impl FileSystem for Disk {
    fn capabilities(&mut self, scheme: &str) -> Option<u32> {
        (scheme == "file").then_some(CAPABILITIES)
    }

    fn stat(&mut self, resource: &Resource) -> Result<Stat> {
        stat_path(path(resource))
    }

    /// Links report their target's type; entries that fail to stat are left out.
    fn read_dir(&mut self, resource: &Resource) -> Result<Vec<(String, u32)>> {
        let folder = path(resource);
        let entries = std::fs::read_dir(folder).map_err(|err| io_error(err, "scandir", folder))?;
        let mut children = Vec::new();
        for entry in entries.flatten() {
            let Ok(child) = stat_path(&entry.path()) else { continue };
            children.push((entry.file_name().to_string_lossy().into_owned(), child.file_type));
        }
        Ok(children)
    }

    /// `openFileForRead`: a folder or anything but a regular file is refused.
    fn read_file(&mut self, resource: &Resource) -> Result<Vec<u8>> {
        let file = path(resource);
        let metadata = std::fs::metadata(file).map_err(|err| io_error(err, "open", file))?;
        if metadata.is_dir() {
            return Err(provider_error(FILE_IS_A_DIRECTORY, "File is a directory"));
        }
        if !metadata.is_file() {
            return Err(provider_error(UNAVAILABLE, "File is not a regular file"));
        }
        std::fs::read(file).map_err(|err| io_error(err, "read", file))
    }

    fn write_file(&mut self, resource: &Resource, content: &[u8], create: bool, overwrite: bool) -> Result<()> {
        let file = path(resource);
        if !create || !overwrite {
            match (file.exists(), overwrite, create) {
                (true, false, _) => return Err(provider_error(FILE_EXISTS, "File already exists")),
                (false, _, false) => return Err(provider_error(FILE_NOT_FOUND, "File does not exist")),
                _ => {}
            }
        }
        std::fs::write(file, content).map_err(|err| io_error(err, "open", file))
    }

    fn mkdir(&mut self, resource: &Resource) -> Result<()> {
        std::fs::create_dir(path(resource)).map_err(|err| io_error(err, "mkdir", path(resource)))
    }

    /// `use_trash` moves the item to the Trash as Electron's `shell.trashItem` (NSFileManager `trashItemAtURL`).
    fn delete(&mut self, resource: &Resource, recursive: bool, use_trash: bool) -> Result<()> {
        let target = path(resource);
        if use_trash {
            let mut context = trash::TrashContext::default();
            trash::macos::TrashContextExtMacos::set_delete_method(&mut context, trash::macos::DeleteMethod::NsFileManager);
            return context.delete(target).map_err(|err| provider_error(UNKNOWN, err.to_string()));
        }
        let is_dir = std::fs::symlink_metadata(target).is_ok_and(|metadata| metadata.is_dir());
        let removed = match (is_dir, recursive) {
            (true, true) => std::fs::remove_dir_all(target),
            (true, false) => std::fs::remove_dir(target),
            (false, _) => std::fs::remove_file(target),
        };
        removed.map_err(|err| io_error(err, if is_dir { "rmdir" } else { "unlink" }, target))
    }

    fn rename(&mut self, from: &Resource, to: &Resource, overwrite: bool) -> Result<()> {
        let (from, to) = (path(from), path(to));
        if from == to {
            return Ok(());
        }
        validate_move_copy(from, to, false, overwrite)?;
        std::fs::rename(from, to).map_err(|err| io_error(err, "rename", from))
    }

    fn copy(&mut self, from: &Resource, to: &Resource, overwrite: bool) -> Result<()> {
        let (from, to) = (path(from), path(to));
        if from == to {
            return Ok(());
        }
        validate_move_copy(from, to, true, overwrite)?;
        copy_entry(from, to)
    }
}

/// The working copy file service with no participants and the setting defaults, for operations that run
/// without an extension host.
impl crate::operations::WorkingCopyFiles for Disk {
    fn participate(&mut self, _: crate::operations::FileOperation, _: &[crate::operations::SourceTarget], _: crate::operations::UndoInfo) {}

    fn did_run(&mut self, _: crate::operations::FileOperation, _: &[crate::operations::SourceTarget]) {}

    fn enable_trash(&mut self) -> bool {
        true
    }

    fn empty_file(&mut self, _: &Resource) -> Vec<u8> {
        Vec::new()
    }
}
