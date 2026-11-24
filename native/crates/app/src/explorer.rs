//! The Explorer's file tree: which folders are expanded, and the visible rows read from disk in the
//! order the reference explorer sorts them (explorerViewer.ts `FileSorter`, `explorer.sortOrder: default`).

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use icu_collator::options::CollatorOptions;
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};

/// The `files.exclude` default (files.contribution.ts), matched by name at any depth.
const EXCLUDED_NAMES: [&str; 6] = [".git", ".svn", ".hg", ".jj", ".DS_Store", "Thumbs.db"];

pub struct FileTree {
    pub root: PathBuf,
    expanded: HashSet<PathBuf>,
    pub selected: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// 1 for the root folder's children, as the reference tree counts depth.
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        Self { root, expanded: HashSet::new(), selected: None }
    }

    /// Expands the folders above `path` and selects it, as `explorer.autoReveal` does for the active editor.
    pub fn reveal(&mut self, path: &Path) {
        if !path.starts_with(&self.root) {
            return;
        }
        for folder in path.ancestors().skip(1).take_while(|folder| *folder != self.root) {
            self.expanded.insert(folder.to_path_buf());
        }
        self.selected = Some(path.to_path_buf());
    }

    pub fn toggle(&mut self, folder: &Path) {
        if !self.expanded.remove(folder) {
            self.expanded.insert(folder.to_path_buf());
        }
    }

    /// Read from disk on every call: no file watcher yet, so a redraw shows the folder as it is now.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.push_children(&self.root, 1, &mut rows);
        rows
    }

    fn push_children(&self, folder: &Path, depth: usize, rows: &mut Vec<Row>) {
        for (path, name, is_dir) in sorted_children(folder) {
            let expanded = is_dir && self.expanded.contains(&path);
            rows.push(Row { path: path.clone(), name, depth, is_dir, expanded });
            if expanded {
                self.push_children(&path, depth + 1, rows);
            }
        }
    }
}

/// Folders first, then names in `compareFileNamesDefault` order.
fn sorted_children(folder: &Path) -> Vec<(PathBuf, String, bool)> {
    // shortcut: an unreadable folder shows no children; the reference shows the error in the tree.
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if EXCLUDED_NAMES.contains(&name.as_str()) {
            continue;
        }
        let is_dir = entry.path().is_dir();
        children.push((entry.path(), name, is_dir));
    }
    children.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| compare_file_names(&a.1, &b.1)));
    children
}

/// comparers.ts `compareFileNamesDefault`: `Intl.Collator(undefined, { numeric: true })`, then the shorter name
/// first, since numeric collation makes `foo1` and `foo01` equal.
pub fn compare_file_names(one: &str, other: &str) -> Ordering {
    static COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
        let mut preferences = CollatorPreferences::default();
        preferences.numeric_ordering = Some(CollationNumericOrdering::True);
        Collator::try_new(preferences, CollatorOptions::default()).expect("the root collation is compiled in")
    });
    COLLATOR.compare(one, other).then_with(|| one.len().cmp(&other.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tree: &FileTree) -> Vec<(String, usize)> {
        tree.rows().into_iter().map(|row| (row.name, row.depth)).collect()
    }

    #[test]
    fn lists_folders_first_and_expands_on_toggle() {
        let root = std::env::temp_dir().join(format!("explorer-test-{}", std::process::id()));
        for dir in ["src", "Docs", ".git"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in ["README.md", "notes.txt", "src/main.ts", ".DS_Store"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let mut tree = FileTree::new(root.clone());
        let row = |name: &str, depth| (name.to_string(), depth);
        assert_eq!(names(&tree), [row("Docs", 1), row("src", 1), row("notes.txt", 1), row("README.md", 1)]);

        tree.toggle(&root.join("src"));
        assert_eq!(names(&tree), [row("Docs", 1), row("src", 1), row("main.ts", 2), row("notes.txt", 1), row("README.md", 1)]);
        assert!(tree.rows()[1].expanded);

        tree.toggle(&root.join("src"));
        assert_eq!(names(&tree).len(), 4);

        tree.reveal(&root.join("src/main.ts"));
        assert_eq!(names(&tree).len(), 5);
        assert_eq!(tree.selected, Some(root.join("src/main.ts")));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Cases from src/vs/base/test/browser/comparers.test.ts `compareFileNamesDefault`.
    #[test]
    fn sorts_names_like_compare_file_names_default() {
        let sorted = |names: &[&'static str]| {
            let mut names = names.to_vec();
            names.sort_by(|a, b| compare_file_names(a, b));
            names
        };
        assert_eq!(sorted(&["file10", "file9", "file1"]), ["file1", "file9", "file10"]);
        assert_eq!(sorted(&["a10.txt", "A2.txt", "A100.txt", "a20.txt"]), ["A2.txt", "a10.txt", "a20.txt", "A100.txt"]);
        assert_eq!(compare_file_names("abc02.txt", "abc010.txt"), Ordering::Less);
        assert_eq!(compare_file_names("abc1.10.txt", "abc1.2.txt"), Ordering::Greater);
        assert_eq!(compare_file_names("abc02.txt", "abc002.txt"), Ordering::Less);
        assert_eq!(compare_file_names("z", "A"), Ordering::Greater);
        assert_eq!(compare_file_names("Z", "a"), Ordering::Greater);
        assert_eq!(compare_file_names("a", "A"), Ordering::Less);
        assert_eq!(compare_file_names("aggregate.go", "aggregate_repo.go"), Ordering::Greater);
        assert_eq!(compare_file_names(".env", ".aaa.env"), Ordering::Greater);
        assert_eq!(compare_file_names(".aaa_env", ".aaa.env"), Ordering::Less);
        assert_eq!(compare_file_names(".env", "aaa"), Ordering::Less);
        assert_eq!(compare_file_names(".MD", "a.md"), Ordering::Less);
        assert_eq!(sorted(&["Émail", "émail", "Email", "email"]), ["email", "Email", "émail", "Émail"]);
    }
}
