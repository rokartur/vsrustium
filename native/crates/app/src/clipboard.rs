//! electron-browser clipboardService.ts (`NativeClipboardService`) for what GPUI's clipboard lacks: the file
//! list Explorer copies (`code/file-list`) and the macOS find pasteboard. Text goes through `cx.write_to_clipboard`.

use std::path::{Path, PathBuf};

use crate::editor_history::{file_path, file_uri_string};

const FILE_FORMAT: &str = "code/file-list";

/// `writeResources`: the files' URIs, one per line, replace the clipboard; an empty list leaves it.
pub fn write_resources(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let uris: Vec<String> = paths.iter().map(|path| file_uri_string(path)).collect();
    pasteboard::write_data(FILE_FORMAT, uris.join("\n").as_bytes());
}

/// `readResources`: the copied files; anything that is not a `file:` URI is dropped (clipboard data is untrusted).
pub fn read_resources() -> Vec<PathBuf> {
    let Some(data) = pasteboard::read_data(FILE_FORMAT) else { return Vec::new() };
    String::from_utf8_lossy(&data).split('\n').filter_map(file_path).collect()
}

/// `hasResources`.
pub fn has_resources() -> bool {
    pasteboard::has(FILE_FORMAT)
}

/// `readFindText` and `writeFindText`.
pub use pasteboard::{read_find_text, write_find_text};

/// `resources.distinctParents`: the paths not inside another one of them.
pub fn distinct_parents(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let inside_another = |path: &Path| paths.iter().any(|other| other != path && path.starts_with(other));
    paths.iter().filter(|path| !inside_another(path)).cloned().collect()
}

#[cfg(not(test))]
mod pasteboard {
    use objc2_app_kit::{NSPasteboard, NSPasteboardNameFind, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSData, NSString};

    /// Electron's `writeBuffer`: the clipboard is cleared, then holds the data under the format as its type.
    pub fn write_data(format: &str, data: &[u8]) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setData_forType(Some(&NSData::with_bytes(data)), &NSString::from_str(format));
    }

    pub fn read_data(format: &str) -> Option<Vec<u8>> {
        NSPasteboard::generalPasteboard().dataForType(&NSString::from_str(format)).map(|data| data.to_vec())
    }

    pub fn has(format: &str) -> bool {
        let types = NSArray::from_retained_slice(&[NSString::from_str(format)]);
        NSPasteboard::generalPasteboard().availableTypeFromArray(&types).is_some()
    }

    pub fn read_find_text() -> String {
        // SAFETY: AppKit's constants are initialized before main.
        let (find, string) = unsafe { (NSPasteboardNameFind, NSPasteboardTypeString) };
        NSPasteboard::pasteboardWithName(find).stringForType(string).map(|text| text.to_string()).unwrap_or_default()
    }

    pub fn write_find_text(text: &str) {
        // SAFETY: AppKit's constants are initialized before main.
        let (find, string) = unsafe { (NSPasteboardNameFind, NSPasteboardTypeString) };
        let pasteboard = NSPasteboard::pasteboardWithName(find);
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), string);
    }
}

/// Tests run in parallel and must not touch the machine's pasteboards: each test thread has its own.
#[cfg(test)]
mod pasteboard {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static GENERAL: RefCell<HashMap<String, Vec<u8>>> = RefCell::default();
        static FIND: RefCell<String> = RefCell::default();
    }

    pub fn write_data(format: &str, data: &[u8]) {
        GENERAL.with_borrow_mut(|general| *general = HashMap::from([(format.to_string(), data.to_vec())]));
    }

    pub fn read_data(format: &str) -> Option<Vec<u8>> {
        GENERAL.with_borrow(|general| general.get(format).cloned())
    }

    pub fn has(format: &str) -> bool {
        GENERAL.with_borrow(|general| general.contains_key(format))
    }

    pub fn read_find_text() -> String {
        FIND.with_borrow(|find| find.clone())
    }

    pub fn write_find_text(text: &str) {
        FIND.with_borrow_mut(|find| *find = text.to_string());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn copied_files_round_trip_as_file_uris_and_other_uris_are_dropped() {
        let paths = vec![PathBuf::from("/tmp/a b/c#.txt"), PathBuf::from("/tmp/d")];
        assert!(!super::has_resources());
        super::write_resources(&[]);
        assert!(!super::has_resources(), "an empty list leaves the clipboard");
        super::write_resources(&paths);
        assert_eq!(super::pasteboard::read_data(super::FILE_FORMAT).unwrap(), b"file:///tmp/a%20b/c%23.txt\nfile:///tmp/d");
        assert_eq!(super::read_resources(), paths);
        super::pasteboard::write_data(super::FILE_FORMAT, b"untitled:Untitled-1\nfile:///tmp/e");
        assert_eq!(super::read_resources(), [PathBuf::from("/tmp/e")]);
    }

    #[test]
    fn distinct_parents_drops_paths_inside_another() {
        let paths = vec![PathBuf::from("/a/b"), PathBuf::from("/a"), PathBuf::from("/ab")];
        assert_eq!(super::distinct_parents(paths), [PathBuf::from("/a"), PathBuf::from("/ab")]);
    }
}
