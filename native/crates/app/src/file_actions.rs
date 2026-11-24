//! The pure parts of the Explorer's file actions (contrib/files/browser/fileActions.ts): new names for pasted
//! files, name validation of the inline input, and the texts of the delete confirmations.

use std::path::Path;
use std::sync::LazyLock;

use regex::{Captures, Regex};

pub use crate::quick_input::Severity;

/// `MAX_CONFIRM_FILES` of platform/dialogs/common/dialogs.ts.
const MAX_CONFIRM_FILES: usize = 10;
/// `Constants.MAX_SAFE_SMALL_INTEGER` (base/common/uint.ts).
const MAX_NUMBER: f64 = (1u64 << 30) as f64;

/// `explorer.incrementalNaming`, without `disabled`, which never increments.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum IncrementalNaming {
    Simple,
    Smart,
}

/// `incrementFileName`.
pub fn increment_file_name(name: &str, is_folder: bool, naming: IncrementalNaming) -> String {
    if naming == IncrementalNaming::Simple {
        let extension = if is_folder { "" } else { extname(name) };
        let prefix = &name[..name.len() - extension.len()];
        // name copy 5(.txt) => name copy 6(.txt), name copy(.txt) => name copy 2(.txt)
        static SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+ copy)( \d+)?$").unwrap());
        if let Some(captures) = SUFFIX.captures(prefix) {
            let copy = &captures[1];
            let number = captures.get(2).map_or(1., |number| parse_int(number.as_str().trim_start()));
            let prefix = match number {
                0. => copy.to_string(),
                number if number < MAX_NUMBER => format!("{copy} {}", number + 1.),
                _ => format!("{copy}{} copy", &captures[2]),
            };
            return prefix + extension;
        }
        return format!("{prefix} copy{extension}");
    }

    static SUFFIX_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(.*[\.\-_])(\d+)(\..*)$").unwrap());
    static PREFIX_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)([\.\-_].*)(\..*)$").unwrap());
    static PREFIX_FILE_NO_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)(\..*)$").unwrap());
    static TRAILING_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)$").unwrap());
    static PREFIX_FOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)(.*)$").unwrap());
    let last_dot = name.rfind('.');
    if !is_folder {
        // file.1.txt => file.2.txt
        if SUFFIX_FILE.is_match(name) {
            return replace(&SUFFIX_FILE, name, |c| format!("{}{}{}", &c[1], next_number(&c[2], ".1"), &c[3]));
        }
        // 1.file.txt => 2.file.txt
        if PREFIX_FILE.is_match(name) {
            return replace(&PREFIX_FILE, name, |c| match bump(&c[1]) {
                Some(next) => format!("{next}{}{}", &c[2], &c[3]),
                None => format!("{}{}.1{}", &c[1], &c[2], &c[3]),
            });
        }
        // 1.txt => 2.txt
        if PREFIX_FILE_NO_NAME.is_match(name) {
            return replace(&PREFIX_FILE_NO_NAME, name, |c| format!("{}{}", next_number(&c[1], ".1"), &c[2]));
        }
        // file.txt => file.1.txt
        if let Some(dot) = last_dot {
            return format!("{}.1{}", &name[..dot], &name[dot..]);
        }
        // 123 => 124
        if TRAILING_NUMBER.is_match(name) {
            return replace(&TRAILING_NUMBER, name, |c| next_number(&c[1], ".1"));
        }
        // file => file1; `(.*)(\d*)$` leaves the digits group empty, since `.*` is greedy.
        return format!("{name}1");
    }
    // folder.1 => folder.2
    if TRAILING_NUMBER.is_match(name) {
        return replace(&TRAILING_NUMBER, name, |c| next_number(&c[1], ".1"));
    }
    // 1.folder => 2.folder
    if PREFIX_FOLDER.is_match(name) {
        return replace(&PREFIX_FOLDER, name, |c| match bump(&c[1]) {
            Some(next) => format!("{next}{}", &c[2]),
            None => format!("{}{}.1", &c[1], &c[2]),
        });
    }
    // file => file.1
    format!("{name}.1")
}

/// `name.replace(regex, fn)`: the first match only.
fn replace(regex: &Regex, name: &str, replacement: impl Fn(&Captures) -> String) -> String {
    regex.replacen(name, 1, |captures: &Captures| replacement(captures)).into_owned()
}

/// The digits plus one, padded to their length, or the digits and `too_big` once they reach the maximum.
fn next_number(digits: &str, too_big: &str) -> String {
    bump(digits).unwrap_or_else(|| format!("{digits}{too_big}"))
}

fn bump(digits: &str) -> Option<String> {
    let number = parse_int(digits);
    (number < MAX_NUMBER).then(|| format!("{:0>width$}", number + 1., width = digits.len()))
}

fn parse_int(digits: &str) -> f64 {
    digits.parse().expect("the regex matched digits")
}

/// `path.posix.extname`: the last dot and what follows, `''` for a leading dot or a name of dots.
fn extname(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(dot) if dot == name.len() - 1 && name[..dot].chars().all(|c| c == '.') => "",
        Some(dot) => &name[dot..],
    }
}

/// `validateFileName` for a name typed in `parent`; `editing` is the renamed item's current name.
/// The parent's children are read from disk and compared ignoring case, as the macOS file system does.
pub fn validate_file_name(parent: &Path, editing: Option<&str>, name: &str) -> Option<(String, Severity)> {
    let name = name.trim_matches('\t').trim_end_matches(['/', '\\']);
    if name.trim().is_empty() {
        return Some(("A file or folder name must be provided.".into(), Severity::Error));
    }
    if name.starts_with(['/', '\\']) {
        return Some(("A file or folder name cannot start with a slash.".into(), Severity::Error));
    }
    let names: Vec<&str> = name.split(['/', '\\']).filter(|name| !name.is_empty()).collect();
    if Some(name) != editing {
        let taken = |child: &str| child.to_lowercase() == name.to_lowercase() && Some(child.to_lowercase()) != editing.map(str::to_lowercase);
        let exists = std::fs::read_dir(parent).into_iter().flatten().flatten().any(|entry| taken(&entry.file_name().to_string_lossy()));
        if exists {
            return Some((format!("A file or folder **{name}** already exists at this location. Please choose a different name."), Severity::Error));
        }
    }
    if !names.iter().all(|name| is_valid_basename(name)) {
        let escaped = name.replace('*', "\\*");
        let shown = match escaped.char_indices().nth(255) {
            Some((end, _)) => format!("{}...", &escaped[..end]),
            None => escaped,
        };
        return Some((format!("The name **{shown}** is not valid as a file or folder name. Please choose a different name."), Severity::Error));
    }
    if names.iter().any(|name| name.starts_with(char::is_whitespace) || name.ends_with(char::is_whitespace)) {
        return Some(("Leading or trailing whitespace detected in file or folder name.".into(), Severity::Warning));
    }
    None
}

/// `isValidBasename` of base/common/extpath.ts on macOS.
fn is_valid_basename(name: &str) -> bool {
    !name.trim().is_empty() && !name.contains('/') && name != "." && name != ".." && name.encode_utf16().count() <= 255
}

/// `getFileNamesMessage`: up to ten names, a line for the rest, and an empty last line.
pub fn file_names_message(names: &[String]) -> String {
    let mut lines: Vec<String> = names.iter().take(MAX_CONFIRM_FILES).cloned().collect();
    match names.len().saturating_sub(MAX_CONFIRM_FILES) {
        0 => {}
        1 => lines.push("...1 additional file not shown".into()),
        more => lines.push(format!("...{more} additional files not shown")),
    }
    lines.push(String::new());
    lines.join("\n")
}

/// A deleted Explorer item: its name and whether it is a folder.
pub struct Deleted {
    pub name: String,
    pub is_dir: bool,
}

/// The message and detail of the delete confirmation: `getMoveToTrashMessage` or `getDeleteMessage`, then the
/// line deleteFiles adds.
pub fn delete_confirmation(items: &[Deleted], use_trash: bool) -> (String, String) {
    let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
    let both = items.iter().any(|item| item.is_dir) && items.iter().any(|item| !item.is_dir);
    let permanently = if use_trash { "" } else { "permanently " };
    let count = items.len();
    let (message, detail) = if both {
        (format!("Are you sure you want to {permanently}delete the following {count} files/directories and their contents?"), file_names_message(&names))
    } else if count > 1 && items[0].is_dir {
        (format!("Are you sure you want to {permanently}delete the following {count} directories and their contents?"), file_names_message(&names))
    } else if count > 1 {
        (format!("Are you sure you want to {permanently}delete the following {count} files?"), file_names_message(&names))
    } else if items[0].is_dir {
        (format!("Are you sure you want to {permanently}delete '{}' and its contents?", items[0].name), String::new())
    } else {
        (format!("Are you sure you want to {permanently}delete '{}'?", items[0].name), String::new())
    };
    let separator = if detail.is_empty() { "" } else { "\n" };
    let last = match (use_trash, count > 1) {
        (true, true) => "You can restore these files from the Trash.",
        (true, false) => "You can restore this file from the Trash.",
        (false, _) => delete_detail(items),
    };
    (message, format!("{detail}{separator}{last}"))
}

/// `deleteDetail` of deleteFiles.
pub fn delete_detail(items: &[Deleted]) -> &'static str {
    match (items.iter().any(|item| item.is_dir), items.len() > 1) {
        (true, _) => "This action is irreversible!",
        (false, true) => "You can restore these files using the Undo command.",
        (false, false) => "You can restore this file using the Undo command.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cases from src/vs/workbench/contrib/files/test/browser/fileActions.test.ts.
    #[test]
    fn increments_file_names_simple() {
        let cases = [
            ("test.js", false, "test copy.js"),
            ("test copy.js", false, "test copy 2.js"),
            ("test copy 005.js", false, "test copy 6.js"),
            ("test copy 9007199254740992.js", false, "test copy 9007199254740992 copy.js"),
            ("copy.js", false, "copy copy.js"),
            ("copy 2.js", false, "copy 2 copy.js"),
            ("test", false, "test copy"),
            ("test.", false, "test copy."),
            (".test", false, ".test copy"),
            ("..test", false, ". copy.test"),
            ("test copy 5", false, "test copy 6"),
            ("test", true, "test copy"),
            ("test copy", true, "test copy 2"),
            ("test copy 005", true, "test copy 6"),
            ("test copy 9007199254740992", true, "test copy 9007199254740992 copy"),
            ("copy", true, "copy copy"),
            ("copy 2", true, "copy 2 copy"),
            ("test.js", true, "test.js copy"),
            ("test.js copy 5", true, "test.js copy 6"),
            ("test copy 0", true, "test copy"),
            ("test copy 1", true, "test copy 2"),
        ];
        for (name, is_folder, expected) in cases {
            assert_eq!(increment_file_name(name, is_folder, IncrementalNaming::Simple), expected, "{name}");
        }
    }

    #[test]
    fn increments_file_names_smart() {
        let cases = [
            ("test.js", false, "test.1.js"),
            ("test", true, "test.1"),
            ("test.1.js", false, "test.2.js"),
            ("test.001.js", false, "test.002.js"),
            ("test.009.js", false, "test.010.js"),
            ("test-1.js", false, "test-2.js"),
            ("test-001.js", false, "test-002.js"),
            ("test-099.js", false, "test-100.js"),
            ("test_1.js", false, "test_2.js"),
            ("test.1", true, "test.2"),
            ("test.001", true, "test.002"),
            ("test-1", true, "test-2"),
            ("test_1", true, "test_2"),
            ("test.9007199254740992.js", false, "test.9007199254740992.1.js"),
            ("test.9007199254740992", true, "test.9007199254740992.1"),
            ("1.test.js", false, "2.test.js"),
            ("1.js", false, "2.js"),
            ("9007199254740992.js", false, "9007199254740992.1.js"),
            ("001.test.js", false, "002.test.js"),
            ("1-test.js", false, "2-test.js"),
            ("1_test.js", false, "2_test.js"),
            ("9007199254740992.test.js", false, "9007199254740992.test.1.js"),
            ("001004", false, "001005"),
            ("9007199254740992", false, "9007199254740992.1"),
            ("file", false, "file1"),
            ("file1", false, "file2"),
            ("file9007199254740992", false, "file9007199254740992.1"),
            ("1.test", true, "2.test"),
            ("9007199254740992.test", true, "9007199254740992.test.1"),
            ("001.test", true, "002.test"),
            ("1-test", true, "2-test"),
        ];
        for (name, is_folder, expected) in cases {
            assert_eq!(increment_file_name(name, is_folder, IncrementalNaming::Smart), expected, "{name}");
        }
    }

    /// `Validate File Name (For Create)`, `(For Rename)` and `Validate Multi-Path File Names` of explorerModel.test.ts.
    #[test]
    fn validates_file_names() {
        let root = std::env::temp_dir().join(format!("file-actions-validate-{}", std::process::id()));
        std::fs::create_dir_all(root.join("path/to/stat")).unwrap();
        std::fs::write(root.join("alles.klar"), "").unwrap();
        std::fs::write(root.join("path/to/stat/fileNested"), "").unwrap();
        let error = |name: &str| validate_file_name(&root, None, name).map(|(_, severity)| severity);
        for name in ["", "  ", "alles.klar", "Alles.klar", "/slashAtBeginning", "/path/to/stat/fileNested/aChild", "/path/to/stat/", ".", "a/../b/.."] {
            assert_eq!(error(name), Some(Severity::Error), "{name:?}");
        }
        for name in ["Read Me", ".foo", "foo.bar", "foo", "foo/bar", "foo\\bar", "all/slashes/are/same", "theres/one/different\\slash", "foo:bar", "foo*bar"] {
            assert_eq!(error(name), None, "{name:?}");
        }
        assert_eq!(error(" foo"), Some(Severity::Warning));
        assert_eq!(validate_file_name(&root, Some("alles.klar"), "alles.klar"), None, "the item keeps its name");
        assert_eq!(validate_file_name(&root, Some("alles.klar"), "Alles.Klar"), None, "a rename may change the case");
        assert_eq!(
            validate_file_name(&root, None, "alles.klar").unwrap().0,
            "A file or folder **alles.klar** already exists at this location. Please choose a different name."
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_confirmations_name_the_items() {
        let file = |name: &str| Deleted { name: name.into(), is_dir: false };
        let folder = |name: &str| Deleted { name: name.into(), is_dir: true };
        assert_eq!(delete_confirmation(&[file("a.txt")], true), ("Are you sure you want to delete 'a.txt'?".into(), "You can restore this file from the Trash.".into()));
        assert_eq!(
            delete_confirmation(&[folder("src")], false),
            ("Are you sure you want to permanently delete 'src' and its contents?".into(), "This action is irreversible!".into())
        );
        assert_eq!(
            delete_confirmation(&[file("a"), file("b")], false),
            ("Are you sure you want to permanently delete the following 2 files?".into(), "a\nb\n\nYou can restore these files using the Undo command.".into())
        );
        assert_eq!(delete_confirmation(&[folder("a"), file("b")], true).0, "Are you sure you want to delete the following 2 files/directories and their contents?");
    }
}
