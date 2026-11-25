//! The Explorer's file tree: which folders are expanded, and the visible rows read from disk in the
//! order the reference explorer sorts them (explorerViewer.ts `FileSorter`, `explorer.sortOrder: default`).

mod nesting;

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use config::configuration::Overrides;
use config::service::Settings;
use config::workspace::Workspace;
use serde_json::{Value, json};

use crate::editor::uri_string;
use nesting::FileNestingTrie;
use icu_collator::options::CollatorOptions;
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};

/// The `files.exclude` default (files.contribution.ts), matched by name at any depth.
const EXCLUDED_NAMES: [&str; 6] = [".git", ".svn", ".hg", ".jj", ".DS_Store", "Thumbs.db"];

pub struct FileTree {
    pub workspace: Workspace,
    /// Read for `explorer.fileNesting.*` of each folder.
    settings: Arc<Mutex<Settings>>,
    expanded: HashSet<PathBuf>,
    /// Nests the user collapsed: with `explorer.fileNesting.expand` the others are expanded (`collapseByDefault`).
    collapsed_nests: HashSet<PathBuf>,
    /// The list's focused row, its selection, and where a Shift range starts.
    pub focused: Option<PathBuf>,
    pub selected: Vec<PathBuf>,
    anchor: Option<PathBuf>,
    /// `IExplorerService.setToCopy`: the items copied or cut, and whether a paste moves them.
    /// shortcut: kept in the window, where desktop writes the OS clipboard (`writeResources`); another window cannot paste them.
    pub clipboard: Vec<PathBuf>,
    pub cut: bool,
    /// `NewExplorerItem`: the row of the name being typed for a new file or folder, under `parent`.
    pub new_item: Option<NewItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewItem {
    pub parent: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// 1 for the top rows, as the reference tree counts depth: the folder's children, or a workspace's folders.
    pub depth: usize,
    pub is_dir: bool,
    /// A file other files are nested under; it has a twistie as a folder does.
    pub has_nests: bool,
    pub expanded: bool,
    /// The new item's row: no name yet, `path` is its parent.
    pub is_new: bool,
}

impl FileTree {
    /// `ExplorerView.doSetTreeInput` without a stored view state: a workspace file shows its folders as roots,
    /// the first five expanded.
    pub fn new(workspace: Workspace, settings: Arc<Mutex<Settings>>) -> Self {
        let mut expanded = HashSet::new();
        if workspace.file.is_some() {
            expanded.extend(workspace.folders.iter().take(5).map(|folder| folder.path.clone()));
        }
        Self { workspace, settings, expanded, collapsed_nests: HashSet::new(), focused: None, selected: Vec::new(), anchor: None, clipboard: Vec::new(), cut: false, new_item: None }
    }

    /// `AsyncDataTree.getViewState` with explorerModel.ts `getId` (`<root URI>::<resource URI>`), as
    /// `ExplorerView.storeTreeViewState` writes it.
    pub fn view_state(&self, scroll_top: f32) -> Value {
        let id = |path: &PathBuf| {
            let root = self.workspace.folder_of(path).map_or(path.as_path(), |folder| folder.path.as_path());
            format!("{}::{}", uri_string("file", root), uri_string("file", path))
        };
        let mut expanded: Vec<&PathBuf> = self.expanded.iter().filter(|path| path.exists()).collect();
        let rows = self.rows();
        expanded.extend(rows.iter().filter(|row| row.has_nests && row.expanded && !self.expanded.contains(&row.path)).map(|row| &row.path));
        json!({
            "focus": self.focused.iter().map(id).collect::<Vec<_>>(),
            "selection": self.selected.iter().map(id).collect::<Vec<_>>(),
            "expanded": expanded.into_iter().map(id).collect::<Vec<_>>(),
            "scrollTop": scroll_top,
        })
    }

    /// `ExplorerView.setTreeInput` with the stored view state: it replaces the default expansion, and focus and
    /// selection keep the items that are shown once the stored folders are expanded. Answers `scrollTop`.
    pub fn restore_view_state(&mut self, state: &Value) -> f32 {
        let paths = |key: &str| -> Vec<PathBuf> {
            let ids = state[key].as_array().map(Vec::as_slice).unwrap_or_default();
            ids.iter().filter_map(|id| crate::editor_history::file_path(id.as_str()?.split("::").nth(1)?)).collect()
        };
        self.expanded = paths("expanded").into_iter().collect();
        let shown: HashSet<PathBuf> = self.rows().into_iter().map(|row| row.path).collect();
        self.focused = paths("focus").into_iter().find(|path| shown.contains(path));
        self.selected = paths("selection").into_iter().filter(|path| shown.contains(path)).collect();
        self.anchor = self.focused.clone();
        state["scrollTop"].as_f64().unwrap_or(0.) as f32
    }

    /// The tree shows the folders as rows, as for any WorkbenchState but FOLDER.
    pub fn shows_roots(&self) -> bool {
        self.workspace.file.is_some()
    }

    /// Expands the folders above `path` and selects it, as `explorer.autoReveal` does for the active editor.
    pub fn reveal(&mut self, path: &Path) {
        let Some(root) = self.workspace.folder_of(path).map(|folder| folder.path.clone()) else { return };
        for folder in path.ancestors().skip(1).take_while(|folder| *folder != root) {
            self.expanded.insert(folder.to_path_buf());
        }
        if self.shows_roots() && path != root {
            self.expanded.insert(root);
        }
        self.set_selection(path);
    }

    /// A plain click: the row alone is selected and focused.
    pub fn set_selection(&mut self, path: &Path) {
        self.focused = Some(path.to_path_buf());
        self.selected = vec![path.to_path_buf()];
        self.anchor = Some(path.to_path_buf());
    }

    /// explorerService.ts `onDidRunOperation` for a move: the moved item and the items under it keep their
    /// focus, selection and expansion, as upstream moves the model element itself.
    pub fn move_item(&mut self, old: &Path, new: &Path) {
        // Joining an empty rest would add a trailing slash.
        let moved = |path: &PathBuf| path.strip_prefix(old).ok().map(|rest| if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) });
        for path in self.focused.iter_mut().chain(self.anchor.iter_mut()).chain(self.selected.iter_mut()) {
            if let Some(path_moved) = moved(path) {
                *path = path_moved;
            }
        }
        for set in [&mut self.expanded, &mut self.collapsed_nests] {
            let moved_paths: Vec<PathBuf> = set.iter().filter_map(moved).collect();
            set.retain(|path| !path.starts_with(old));
            set.extend(moved_paths);
        }
    }

    /// A right click: listWidget.ts `onViewPointer` focuses the row, or clears focus and selection outside the
    /// rows; explorerView.ts `onContextMenu` then selects the row if the selection does not hold it.
    pub fn context_menu_select(&mut self, path: Option<&Path>) {
        let Some(path) = path else {
            self.focused = None;
            self.selected.clear();
            self.anchor = None;
            return;
        };
        self.focused = Some(path.to_path_buf());
        self.anchor = Some(path.to_path_buf());
        if !self.selected.iter().any(|selected| selected == path) {
            self.selected = vec![path.to_path_buf()];
        }
    }

    /// listWidget.ts `MouseController`: Cmd+click toggles the row in the selection, Shift+click selects the
    /// visible rows from the anchor to it.
    pub fn click_select(&mut self, path: &Path, toggle: bool, range: bool) {
        let rows: Vec<PathBuf> = self.rows().into_iter().map(|row| row.path).collect();
        let index = |path: &Path| rows.iter().position(|row| row == path);
        match (range, self.anchor.as_deref().and_then(index), index(path)) {
            (true, Some(anchor), Some(clicked)) => {
                self.selected = rows[anchor.min(clicked)..=anchor.max(clicked)].to_vec();
                self.focused = Some(path.to_path_buf());
            }
            _ if toggle => {
                match self.selected.iter().position(|selected| selected == path) {
                    Some(position) => {
                        self.selected.remove(position);
                    }
                    None => self.selected.push(path.to_path_buf()),
                }
                self.focused = Some(path.to_path_buf());
                self.anchor = Some(path.to_path_buf());
            }
            _ => self.set_selection(path),
        }
    }

    /// `ExplorerService.getContext`: the selection when it holds the focused row and `respect_multi_selection`,
    /// else the focused row.
    /// With `respect_multi_selection` and not `ignore_nested_children`, a collapsed nest brings the files nested under it.
    pub fn context(&self, respect_multi_selection: bool, ignore_nested_children: bool) -> Vec<PathBuf> {
        let Some(focused) = &self.focused else { return Vec::new() };
        if !respect_multi_selection || !self.selected.contains(focused) {
            return vec![focused.clone()];
        }
        let mut items = self.selected.clone();
        for item in self.selected.iter().filter(|_| !ignore_nested_children) {
            let Some(folder) = item.parent().filter(|_| !item.is_dir()) else { continue };
            let Some(entry) = self.entries(folder).into_iter().find(|entry| entry.path == *item) else { continue };
            if !entry.nested.is_empty() && !self.nest_expanded(item) {
                items.extend(entry.nested.into_iter().map(|(path, _)| path).filter(|path| !self.selected.contains(path)));
            }
        }
        items
    }

    /// A workspace folder: the tree's root, a row only in a workspace.
    pub fn is_root(&self, path: &Path) -> bool {
        self.workspace.folders.iter().any(|folder| folder.path == path)
    }

    /// Where a new or pasted item goes for the item it was invoked on: the folder itself, else its parent.
    pub fn folder_for(&self, path: &Path) -> PathBuf {
        match path.is_dir() {
            true => path.to_path_buf(),
            false => path.parent().expect("an Explorer item has a parent").to_path_buf(),
        }
    }

    pub fn expand(&mut self, folder: &Path) {
        self.expanded.insert(folder.to_path_buf());
    }

    /// `ExplorerService.isCut`.
    pub fn is_expanded(&self, folder: &Path) -> bool {
        self.expanded.contains(folder)
    }

    pub fn is_cut(&self, path: &Path) -> bool {
        self.cut && self.clipboard.iter().any(|cut| cut == path)
    }

    pub fn toggle(&mut self, folder: &Path) {
        let expanded = self.expanded.contains(folder);
        self.set_expanded(folder, !expanded);
    }

    pub fn set_expanded(&mut self, path: &Path, expanded: bool) {
        match expanded {
            true => {
                self.expanded.insert(path.to_path_buf());
                self.collapsed_nests.remove(path);
            }
            false => {
                self.expanded.remove(path);
                self.collapsed_nests.insert(path.to_path_buf());
            }
        }
    }

    fn nest_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path) || (self.nesting_setting(path, "explorer.fileNesting.expand").as_bool() == Some(true) && !self.collapsed_nests.contains(path))
    }

    /// Read from disk on every call: no file watcher yet, so a redraw shows the folder as it is now.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if !self.shows_roots() {
            self.push_children(&self.workspace.folders[0].path, 1, &mut rows);
            return rows;
        }
        for folder in &self.workspace.folders {
            let expanded = self.expanded.contains(&folder.path);
            rows.push(Row { path: folder.path.clone(), name: folder.name.clone(), depth: 1, is_dir: true, has_nests: false, expanded, is_new: false });
            if expanded {
                self.push_children(&folder.path, 2, &mut rows);
            }
        }
        rows
    }

    /// The new item has an empty name, so `FileSorter` puts it first among the folders or the files.
    fn push_children(&self, folder: &Path, depth: usize, rows: &mut Vec<Row>) {
        let new_item = self.new_item.as_ref().filter(|item| item.parent == folder);
        let new_row = |is_dir| Row { path: folder.to_path_buf(), name: String::new(), depth, is_dir, has_nests: false, expanded: false, is_new: true };
        if new_item.is_some_and(|item| item.is_dir) {
            rows.push(new_row(true));
        }
        let mut new_file = new_item.filter(|item| !item.is_dir);
        for Entry { path, name, is_dir, nested } in self.entries(folder) {
            if !is_dir && new_file.take().is_some() {
                rows.push(new_row(false));
            }
            let has_nests = !nested.is_empty();
            let expanded = match is_dir {
                true => self.expanded.contains(&path),
                false => has_nests && self.nest_expanded(&path),
            };
            rows.push(Row { path: path.clone(), name, depth, is_dir, has_nests, expanded, is_new: false });
            if expanded && is_dir {
                self.push_children(&path, depth + 1, rows);
            }
            if expanded && has_nests {
                rows.extend(nested.into_iter().map(|(path, name)| Row { path, name, depth: depth + 1, is_dir: false, has_nests: false, expanded: false, is_new: false }));
            }
        }
        if new_file.is_some() {
            rows.push(new_row(false));
        }
    }

    /// The items of `folder` in `FileSorter` order. With `explorer.fileNesting.enabled`, a file nested under another
    /// is left out and listed, sorted, in its parent's `nested` (explorerModel.ts `fetchChildren`). Names are
    /// matched lowercased, as `getPlatformAwareName` on a disk without PathCaseSensitive.
    fn entries(&self, folder: &Path) -> Vec<Entry> {
        let children = sorted_children(folder);
        let visible = |name: &str| !EXCLUDED_NAMES.contains(&name);
        let Some(nester) = self.nester(folder) else {
            return children.into_iter().filter(|(_, name, _)| visible(name)).map(|(path, name, is_dir)| Entry { path, name, is_dir, nested: Vec::new() }).collect();
        };
        // `files.exclude` filters the tree after nesting, so an excluded file still nests the files under it.
        let files: Vec<String> = children.iter().filter(|(_, _, is_dir)| !is_dir).map(|(_, name, _)| name.to_lowercase()).collect();
        let dirname = folder.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        let nests = nester.nest(&files, &dirname);
        let mut entries = Vec::new();
        for (path, name, is_dir) in &children {
            if !visible(name) {
                continue;
            }
            if *is_dir {
                entries.push(Entry { path: path.clone(), name: name.clone(), is_dir: true, nested: Vec::new() });
                continue;
            }
            let Some(nested_names) = nests.get(&name.to_lowercase()) else { continue };
            let nested = children
                .iter()
                .filter(|(_, child, is_dir)| !is_dir && visible(child) && nested_names.contains(&child.to_lowercase()))
                .map(|(path, name, _)| (path.clone(), name.clone()))
                .collect();
            entries.push(Entry { path: path.clone(), name: name.clone(), is_dir: false, nested });
        }
        entries
    }

    /// explorerModel.ts `fileNester` for the folder's `explorer.fileNesting.patterns`; `None` while nesting is off.
    fn nester(&self, folder: &Path) -> Option<FileNestingTrie> {
        if self.nesting_setting(folder, "explorer.fileNesting.enabled").as_bool() != Some(true) {
            return None;
        }
        let patterns = self.nesting_setting(folder, "explorer.fileNesting.patterns");
        let mut config = Vec::new();
        for (parent, children) in patterns.as_object().into_iter().flatten() {
            let Some(children) = children.as_str().filter(|children| !parent.is_empty() && !children.is_empty()) else { continue };
            let children = children.split(',').map(|child| child.trim().replace('\u{200b}', "").trim().to_lowercase()).filter(|child| !child.is_empty()).collect();
            config.push((parent.trim().to_lowercase(), children));
        }
        Some(FileNestingTrie::new(&config))
    }

    fn nesting_setting(&self, resource: &Path, key: &str) -> Value {
        self.settings.lock().unwrap().get_value(Some(key), &Overrides { resource: Some(resource), ..Overrides::default() }).unwrap_or(Value::Null)
    }
}

/// An item of a folder, with the files nested under it.
struct Entry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    nested: Vec<(PathBuf, String)>,
}

/// Folders first, then names in `compareFileNamesDefault` order; excluded names too.
fn sorted_children(folder: &Path) -> Vec<(PathBuf, String, bool)> {
    // shortcut: an unreadable folder shows no children; the reference shows the error in the tree.
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
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
        let mut tree = FileTree::new(Workspace::folder(root.clone()), crate::default_settings());
        let row = |name: &str, depth| (name.to_string(), depth);
        assert_eq!(names(&tree), [row("Docs", 1), row("src", 1), row("notes.txt", 1), row("README.md", 1)]);

        tree.toggle(&root.join("src"));
        assert_eq!(names(&tree), [row("Docs", 1), row("src", 1), row("main.ts", 2), row("notes.txt", 1), row("README.md", 1)]);
        assert!(tree.rows()[1].expanded);

        tree.new_item = Some(NewItem { parent: root.join("src"), is_dir: false });
        let new_row = tree.rows().into_iter().position(|row| row.is_new);
        assert_eq!(new_row, Some(2), "a new file goes first among the files");
        tree.new_item = Some(NewItem { parent: root.clone(), is_dir: true });
        assert_eq!(names(&tree)[0], row("", 1), "a new folder goes first among the folders");
        tree.new_item = None;

        tree.toggle(&root.join("src"));
        assert_eq!(names(&tree).len(), 4);

        tree.reveal(&root.join("src/main.ts"));
        assert_eq!(names(&tree).len(), 5);
        assert_eq!(tree.selected, [root.join("src/main.ts")]);

        tree.click_select(&root.join("Docs"), false, true);
        assert_eq!(tree.selected.len(), 3, "Shift selects from the anchor");
        tree.click_select(&root.join("src"), true, false);
        assert_eq!(tree.selected, [root.join("Docs"), root.join("src/main.ts")], "Cmd toggles a row");
        assert_eq!(tree.context(true, false), [root.join("src")], "the focused row outside the selection is the context");
        tree.click_select(&root.join("Docs"), true, false);
        tree.click_select(&root.join("Docs"), true, false);
        assert_eq!(tree.context(true, false), [root.join("src/main.ts"), root.join("Docs")]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_workspace_shows_its_folders_as_roots() {
        let root = std::env::temp_dir().join(format!("explorer-roots-test-{}", std::process::id()));
        let folders: Vec<String> = (1..=6).map(|index| format!("folder{index}")).collect();
        for folder in &folders {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join("a.txt"), "").unwrap();
        }
        let configured: Vec<String> = folders.iter().map(|folder| format!(r#"{{ "path": "{folder}" }}"#)).collect();
        let file = root.join("test.code-workspace");
        std::fs::write(&file, format!(r#"{{ "folders": [{}, {{ "path": "folder1/../folder1" }}] }}"#, configured.join(", "))).unwrap();
        let mut tree = FileTree::new(Workspace::from_file(file), crate::default_settings());
        let rows = names(&tree);
        let row = |name: &str, depth| (name.to_string(), depth);
        assert_eq!(rows[..3], [row("folder1", 1), row("a.txt", 2), row("folder2", 1)]);
        assert_eq!(rows[rows.len() - 2..], [row("a.txt", 2), row("folder6", 1)], "only the first five roots are expanded");

        tree.toggle(&root.join("folder1"));
        tree.reveal(&root.join("folder1/a.txt"));
        assert!(tree.rows()[0].expanded, "revealing a file expands its root");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_moved_folder_keeps_selection_and_expansion_of_its_items() {
        let mut tree = FileTree::new(Workspace::folder(PathBuf::from("/w")), crate::default_settings());
        tree.toggle(Path::new("/w/src"));
        tree.toggle(Path::new("/w/src/lib"));
        tree.click_select(Path::new("/w/src/lib/a.ts"), false, false);
        tree.click_select(Path::new("/w/srcfile"), true, false);
        tree.move_item(Path::new("/w/src"), Path::new("/w/out/src"));
        assert_eq!(tree.selected, [PathBuf::from("/w/out/src/lib/a.ts"), PathBuf::from("/w/srcfile")], "a name that only starts with the folder's stays");
        assert!(tree.expanded.contains(Path::new("/w/out/src")) && tree.expanded.contains(Path::new("/w/out/src/lib")));
        assert!(!tree.expanded.iter().any(|path| path.starts_with("/w/src")));
    }

    #[test]
    fn nests_files_under_their_parent_with_file_nesting() {
        let root = std::env::temp_dir().join(format!("explorer-nesting-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        for file in ["a.ts", "a.js", "a.js.map", "b.js", "package.json", "Package-Lock.json", ".DS_Store"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let user = root.with_extension("user");
        std::fs::create_dir_all(&user).unwrap();
        let settings = Arc::new(Mutex::new(Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None)));
        let mut tree = FileTree::new(Workspace::folder(root.clone()), settings.clone());
        assert_eq!(names(&tree).len(), 7, "nesting is off by default");

        settings.lock().unwrap().update_value_derived("explorer.fileNesting.enabled", Some(&json!(true))).unwrap();
        let rows = tree.rows();
        let shown: Vec<(&str, usize, bool)> = rows.iter().map(|row| (row.name.as_str(), row.depth, row.has_nests)).collect();
        assert_eq!(
            shown,
            [("src", 1, false), ("a.ts", 1, true), ("a.js", 2, false), ("a.js.map", 2, false), ("b.js", 1, false), ("package.json", 1, true), ("Package-Lock.json", 2, false)],
            "the default patterns nest a.js and, through it, a.js.map under a.ts; names match without case"
        );
        assert!(rows[1].expanded, "explorer.fileNesting.expand defaults to true");

        tree.set_expanded(&root.join("a.ts"), false);
        assert_eq!(names(&tree).len(), 5);
        tree.click_select(&root.join("a.ts"), false, false);
        assert_eq!(tree.context(true, false), [root.join("a.ts"), root.join("a.js"), root.join("a.js.map")], "a collapsed nest brings its files");
        assert_eq!(tree.context(true, true), [root.join("a.ts")]);
        assert_eq!(tree.context(false, false), [root.join("a.ts")]);

        settings.lock().unwrap().update_value_derived("explorer.fileNesting.expand", Some(&json!(false))).unwrap();
        tree.set_expanded(&root.join("a.ts"), true);
        assert_eq!(names(&tree).len(), 6, "package.json starts collapsed, a.ts is expanded again");
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(user).unwrap();
    }

    #[test]
    fn restores_the_stored_view_state() {
        let root = std::env::temp_dir().join(format!("explorer-view-state-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/lib")).unwrap();
        std::fs::write(root.join("src/lib/a b.ts"), "").unwrap();
        let mut tree = FileTree::new(Workspace::folder(root.clone()), crate::default_settings());
        tree.reveal(&root.join("src/lib/a b.ts"));
        let state = tree.view_state(44.);
        let root_uri = uri_string("file", &root);
        assert_eq!(state["focus"], json!([format!("{root_uri}::{root_uri}/src/lib/a%20b.ts")]), "ids as explorerModel.ts getId");
        assert_eq!(state["scrollTop"], json!(44.));

        let mut reopened = FileTree::new(Workspace::folder(root.clone()), crate::default_settings());
        assert_eq!(reopened.restore_view_state(&state), 44.);
        assert_eq!(names(&reopened), names(&tree));
        assert_eq!(reopened.selected, [root.join("src/lib/a b.ts")]);
        assert_eq!(reopened.context(false, false), [root.join("src/lib/a b.ts")]);

        std::fs::remove_dir_all(root.join("src/lib")).unwrap();
        let mut reopened = FileTree::new(Workspace::folder(root.clone()), crate::default_settings());
        reopened.restore_view_state(&state);
        assert!(reopened.focused.is_none() && reopened.selected.is_empty(), "items no longer shown are dropped");
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
