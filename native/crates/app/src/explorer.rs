//! The Explorer's file tree: which folders are expanded, and the visible rows read from disk in the
//! order the reference explorer sorts them (explorerViewer.ts `FileSorter`, `explorer.sortOrder: default`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

/// Folders first, then names compared case-insensitively.
/// shortcut: no numeric collation ("file10" sorts before "file9"), port compareFileNamesDefault when it matters.
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
    children.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())).then_with(|| a.1.cmp(&b.1)));
    children
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
}
