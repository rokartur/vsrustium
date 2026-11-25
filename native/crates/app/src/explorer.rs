//! The Explorer's file tree: which folders are expanded, and the visible rows read from disk in the
//! order the reference explorer sorts them (explorerViewer.ts `FileSorter` with the `explorer.sortOrder` settings).

mod nesting;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use config::configuration::Overrides;
use config::service::Settings;
use config::workspace::Workspace;
use serde_json::{Value, json};

use crate::editor::uri_string;
use nesting::FileNestingTrie;
use icu_collator::options::{CollatorOptions, Strength};
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
    /// `IExplorerService.setToCopy`: the items copied or cut, and whether a paste moves them (`pasteShouldMove`).
    /// The clipboard holds their URIs (`writeResources`).
    pub clipboard: Vec<PathBuf>,
    pub cut: bool,
    /// `fileCopied`: set by a copy or cut, and from `hasResources` when the context menu opens.
    pub file_copied: bool,
    /// `NewExplorerItem`: the row of the name being typed for a new file or folder, under `parent`.
    pub new_item: Option<NewItem>,
    /// `CompressedNavigationController.index` of compact rows, keyed by the row's last folder; a row not here is at its last.
    compressed_index: HashMap<PathBuf, usize>,
    /// explorerViewer.ts `ExplorerFindProvider` filter session: only the items `markItemAndParentsAsFiltered` marked
    /// show, and the view state of the session's start comes back when it ends.
    filter: Option<Filter>,
}

struct Filter {
    marked: HashSet<PathBuf>,
    start_state: Value,
    expanded_before: HashSet<PathBuf>,
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
    /// The folders of a compact row (`explorer.compactFolders`), first to last; `path` and `name` are the last's.
    /// Empty for any other row.
    pub compressed: Vec<(PathBuf, String)>,
}

/// explorerViewer.ts `CompressedNavigationController`: the folders of a compact row and the one keyboard and
/// context menu commands act on.
#[derive(Clone, Debug, PartialEq)]
pub struct Navigation {
    pub items: Vec<PathBuf>,
    pub index: usize,
}

impl Navigation {
    pub fn current(&self) -> &PathBuf {
        &self.items[self.index]
    }

    pub fn previous(&mut self) {
        self.set_index(self.index.saturating_sub(1));
    }

    pub fn next(&mut self) {
        self.set_index(self.index + 1);
    }

    pub fn first(&mut self) {
        self.index = 0;
    }

    pub fn last(&mut self) {
        self.index = self.items.len() - 1;
    }

    pub fn set_index(&mut self, index: usize) {
        if index < self.items.len() {
            self.index = index;
        }
    }
}

/// explorerView.ts `getContext`: with a multi-selection and `respect_multi_selection` the selection, else the focused
/// item when it is selected (with the selection) or alone. A compact row stands for its navigation's current folder
/// when focused, for all its folders when only selected.
pub fn get_context(focus: Option<&Path>, selection: &[PathBuf], respect_multi_selection: bool, navigation: impl Fn(&Path) -> Option<Navigation>) -> Vec<PathBuf> {
    let focus = focus.filter(|_| !(respect_multi_selection && selection.len() > 1));
    let focused_navigation = focus.and_then(&navigation);
    let focused = match &focused_navigation {
        Some(navigation) => Some(navigation.current().clone()),
        None => focus.map(Path::to_path_buf),
    };
    let mut selected = Vec::new();
    for stat in selection {
        match navigation(stat) {
            // Folders of the focused compact row other than its current one are left out.
            Some(stat_navigation) if focused.is_some() && Some(&stat_navigation) == focused_navigation.as_ref() => {
                if Some(stat) == focused.as_ref() {
                    selected.push(stat.clone());
                }
            }
            Some(stat_navigation) => selected.extend(stat_navigation.items),
            None => selected.push(stat.clone()),
        }
    }
    let Some(focused) = focused else {
        return if respect_multi_selection { selected } else { Vec::new() };
    };
    if respect_multi_selection && selected.contains(&focused) {
        return selected;
    }
    vec![focused]
}

impl FileTree {
    /// `ExplorerView.doSetTreeInput` without a stored view state: a workspace file shows its folders as roots,
    /// the first five expanded.
    pub fn new(workspace: Workspace, settings: Arc<Mutex<Settings>>) -> Self {
        let mut expanded = HashSet::new();
        if workspace.file.is_some() {
            expanded.extend(workspace.folders.iter().take(5).map(|folder| folder.path.clone()));
        }
        Self {
            workspace,
            settings,
            expanded,
            collapsed_nests: HashSet::new(),
            focused: None,
            selected: Vec::new(),
            anchor: None,
            clipboard: Vec::new(),
            cut: false,
            file_copied: false,
            new_item: None,
            compressed_index: HashMap::new(),
            filter: None,
        }
    }

    /// `AsyncDataTree.getViewState` with explorerModel.ts `getId` (`<root URI>::<resource URI>`), as
    /// `ExplorerView.storeTreeViewState` writes it.
    pub fn view_state(&self, scroll_top: f32) -> Value {
        let id = |path: &PathBuf| {
            let root = self.workspace.folder_of(path).map_or(path.as_path(), |folder| folder.path.as_path());
            format!("{}::{}", uri_string("file", root), uri_string("file", path))
        };
        let rows = self.rows();
        // `CompressibleAsyncDataTree.getViewState`: a collapsed compact row lists none of its folders.
        let collapsed_compact: HashSet<&PathBuf> = rows.iter().filter(|row| !row.expanded).flat_map(|row| row.compressed.iter().map(|(path, _)| path)).collect();
        let mut expanded: Vec<&PathBuf> = self.expanded.iter().filter(|path| path.exists() && !collapsed_compact.contains(path)).collect();
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
        // A folder inside a compact row selects the row, as the compressed tree maps each of its elements to the row.
        let row = self.rows().into_iter().find(|row| row.compressed.iter().any(|(folder, _)| folder == path)).map(|row| row.path);
        self.set_selection(row.as_deref().unwrap_or(path));
    }

    /// A plain click: the row alone is selected and focused.
    pub fn set_selection(&mut self, path: &Path) {
        self.focused = Some(path.to_path_buf());
        self.selected = vec![path.to_path_buf()];
        self.anchor = Some(path.to_path_buf());
    }

    /// A keyboard move: the focus and the anchor go to `path`, the selection stays (listCommands.ts `navigate`).
    pub fn focus(&mut self, path: Option<PathBuf>) {
        self.anchor = path.clone();
        self.focused = path;
    }

    pub fn clear_anchor(&mut self) {
        self.anchor = None;
    }

    /// `list.collapseAll`: every folder and nest collapses.
    pub fn collapse_all(&mut self) {
        let expanded: Vec<PathBuf> = self.rows().into_iter().filter(|row| row.expanded).map(|row| row.path).collect();
        for path in expanded {
            self.set_expanded(&path, false);
        }
        self.expanded.clear();
    }

    /// explorerView.ts `collapseAll`: in a workspace with a root's child expanded, the expanded roots stay open.
    pub fn collapse_folders(&mut self) {
        let open_roots: Vec<PathBuf> = self.workspace.folders.iter().map(|folder| folder.path.clone()).filter(|root| self.expanded.contains(root)).collect();
        let root_child_expanded = self.shows_roots() && self.expanded.iter().any(|path| path.parent().is_some_and(|parent| open_roots.iter().any(|root| root == parent)));
        self.collapse_all();
        if root_child_expanded {
            for root in open_roots {
                self.collapsed_nests.remove(&root);
                self.expanded.insert(root);
            }
        }
    }

    /// listCommands.ts `list.selectAll` on a tree: the shown rows under the parent of the focused row (or of the
    /// selection's first), with the parent itself when they were all selected already.
    pub fn select_all(&mut self) {
        let rows = self.rows();
        let start = match &self.focused {
            Some(focused) if self.selected.is_empty() || !self.selected.contains(focused) => Some(focused.clone()),
            _ => self.selected.first().cloned(),
        };
        let start = start.and_then(|start| rows.iter().position(|row| row.path == start));
        let scope = start.and_then(|start| rows[..start].iter().rposition(|row| row.depth < rows[start].depth));
        let (first, depth) = match scope {
            Some(scope) => (scope + 1, rows[scope].depth),
            None => (0, 0),
        };
        let mut selection: Vec<PathBuf> = rows[first..].iter().take_while(|row| row.depth > depth).map(|row| row.path.clone()).collect();
        if let Some(scope) = scope.filter(|_| self.selected.len() == selection.len()) {
            selection.insert(0, rows[scope].path.clone());
        }
        self.selected = selection;
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

    /// `ExplorerService.getContext` over explorerView.ts `getContext`.
    /// With `respect_multi_selection` and not `ignore_nested_children`, a collapsed nest brings the files nested under it.
    pub fn context(&self, respect_multi_selection: bool, ignore_nested_children: bool) -> Vec<PathBuf> {
        let rows = self.rows();
        let navigation = |path: &Path| rows.iter().find(|row| row.path == path).and_then(|row| self.navigation(row));
        let context = get_context(self.focused.as_deref(), &self.selected, respect_multi_selection, navigation);
        let mut items = Vec::new();
        for item in context {
            if items.contains(&item) {
                continue;
            }
            let nested = match (respect_multi_selection && !ignore_nested_children, item.parent().filter(|_| !item.is_dir())) {
                (true, Some(folder)) => self.entries(folder).into_iter().find(|entry| entry.child.path == item).filter(|_| !self.nest_expanded(&item)).map(|entry| entry.nested),
                _ => None,
            };
            items.push(item);
            items.extend(nested.into_iter().flatten().map(|child| child.path));
        }
        items.dedup();
        items
    }

    /// The navigation of a compact row, at the folder the user last picked in it.
    pub fn navigation(&self, row: &Row) -> Option<Navigation> {
        if row.compressed.is_empty() {
            return None;
        }
        let items: Vec<PathBuf> = row.compressed.iter().map(|(path, _)| path.clone()).collect();
        let index = self.compressed_index.get(&row.path).copied().filter(|index| *index < items.len()).unwrap_or(items.len() - 1);
        Some(Navigation { items, index })
    }

    /// The navigation of the row at `path`, if it is a compact one.
    pub fn navigation_at(&self, path: &Path) -> Option<Navigation> {
        self.rows().iter().find(|row| row.path == path).and_then(|row| self.navigation(row))
    }

    /// Stores a navigation moved by a key, a click on one of its folders or a context menu.
    pub fn set_navigation(&mut self, navigation: &Navigation) {
        let row = navigation.items.last().expect("a compact row has folders").clone();
        self.compressed_index.insert(row, navigation.index);
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
        self.set_expanded(folder, true);
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
                self.expand_single_child_folders(path);
                self.select_compact_rows();
            }
            false => {
                self.expanded.remove(path);
                self.collapsed_nests.insert(path.to_path_buf());
            }
        }
    }

    /// A folder that just joined a compact row is that row in focus and selection, as the compressed tree maps each
    /// of its elements to the row.
    fn select_compact_rows(&mut self) {
        let rows = self.rows();
        let row_of = |path: &mut PathBuf| {
            if let Some(row) = rows.iter().find(|row| row.compressed.iter().any(|(folder, _)| folder == path)) {
                *path = row.path.clone();
            }
        };
        self.focused.iter_mut().chain(self.anchor.iter_mut()).chain(self.selected.iter_mut()).for_each(row_of);
        self.selected.dedup();
    }

    /// asyncDataTree.ts `shouldAutoExpandSingleChild` with `autoExpandCompressedChildren`: a folder's only child, a
    /// folder that compresses with it, is expanded with it, since the compact row shows its children only then.
    fn expand_single_child_folders(&mut self, folder: &Path) {
        if !self.compact_folders() || self.is_root(folder) || !folder.is_dir() {
            return;
        }
        let mut folder = folder.to_path_buf();
        loop {
            let children = read_children(&folder);
            let [Child { path, is_dir: true, .. }] = children.as_slice() else { return };
            folder = path.clone();
            self.expanded.insert(folder.clone());
        }
    }

    /// `explorer.compactFolders`; explorerView.ts also turns it off for screen readers, which the port does not detect.
    fn compact_folders(&self) -> bool {
        self.settings.lock().unwrap().get_value(Some("explorer.compactFolders"), &Overrides::default()) == Some(Value::Bool(true))
    }

    fn nest_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path) || (self.nesting_setting(path, "explorer.fileNesting.expand").as_bool() == Some(true) && !self.collapsed_nests.contains(path))
    }

    /// `doFilterFind`: shows only the `marked` items, every marked folder expanded, as the tree's new input
    /// creates its nodes with `collapseByDefault` false while filter results show.
    /// `scroll_top` is the tree's at the session's start.
    pub fn set_filter(&mut self, marked: HashSet<PathBuf>, scroll_top: f32) {
        let (start_state, expanded_before) = match self.filter.take() {
            Some(filter) => (filter.start_state, filter.expanded_before),
            None => (self.view_state(scroll_top), self.expanded.clone()),
        };
        self.expanded = expanded_before.clone();
        self.expanded.extend(marked.iter().filter(|path| path.is_dir()).cloned());
        self.filter = Some(Filter { marked, start_state, expanded_before });
    }

    /// `endFilterSession`: every item shows again, with the view state of the session's start; returns its scroll top.
    pub fn end_filter(&mut self) -> Option<f32> {
        let filter = self.filter.take()?;
        Some(self.restore_view_state(&filter.start_state))
    }

    pub fn is_filtering(&self) -> bool {
        self.filter.is_some()
    }

    fn is_filtered_out(&self, path: &Path) -> bool {
        self.filter.as_ref().is_some_and(|filter| !filter.marked.contains(path))
    }

    /// Read from disk on every call: no file watcher yet, so a redraw shows the folder as it is now.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if !self.shows_roots() {
            self.push_children(&self.workspace.folders[0].path, 1, &mut rows);
            return rows;
        }
        for folder in &self.workspace.folders {
            if self.is_filtered_out(&folder.path) {
                continue;
            }
            let expanded = self.expanded.contains(&folder.path);
            rows.push(Row { path: folder.path.clone(), name: folder.name.clone(), depth: 1, is_dir: true, has_nests: false, expanded, is_new: false, compressed: Vec::new() });
            if expanded {
                self.push_children(&folder.path, 2, &mut rows);
            }
        }
        rows
    }

    fn push_children(&self, folder: &Path, depth: usize, rows: &mut Vec<Row>) {
        let compact = self.compact_folders();
        for Entry { child: Child { path, name, is_dir, .. }, nested, is_new } in self.entries(folder) {
            if is_new {
                rows.push(Row { path, name, depth, is_dir, has_nests: false, expanded: false, is_new: true, compressed: Vec::new() });
                continue;
            }
            if self.is_filtered_out(&path) {
                continue;
            }
            let has_nests = !nested.is_empty();
            let compressed = match is_dir && compact {
                true => self.compress(&path, &name),
                false => Vec::new(),
            };
            let (path, name) = compressed.last().cloned().unwrap_or((path, name));
            let expanded = match is_dir {
                true => self.expanded.contains(&path),
                false => has_nests && self.nest_expanded(&path),
            };
            rows.push(Row { path: path.clone(), name, depth, is_dir, has_nests, expanded, is_new: false, compressed });
            if expanded && is_dir {
                self.push_children(&path, depth + 1, rows);
            }
            if expanded && has_nests {
                rows.extend(nested.into_iter().filter(|child| !self.is_filtered_out(&child.path)).map(|Child { path, name, .. }| Row { path, name, depth: depth + 1, is_dir: false, has_nests: false, expanded: false, is_new: false, compressed: Vec::new() }));
            }
        }
    }

    /// compressedObjectTreeModel.ts `compress` with `ExplorerCompressionDelegate`: a folder whose children are loaded
    /// (it is expanded) and are one folder takes that folder onto its row, and so on down. Children count unfiltered,
    /// so an excluded `.DS_Store` or a new item keeps them apart. Answers the row's folders, empty for a plain row.
    fn compress(&self, folder: &Path, name: &str) -> Vec<(PathBuf, String)> {
        let mut elements = vec![(folder.to_path_buf(), name.to_string())];
        loop {
            let last = &elements[elements.len() - 1].0;
            if !self.expanded.contains(last) || self.new_item.as_ref().is_some_and(|item| item.parent == *last) {
                break;
            }
            let mut children = read_children(last);
            if children.len() != 1 || !children[0].is_dir {
                break;
            }
            let child = children.remove(0);
            elements.push((child.path, child.name));
        }
        if elements.len() == 1 {
            elements.clear();
        }
        elements
    }

    /// The items of `folder` in `FileSorter` order, with the new item, whose name is empty, among them. With
    /// `explorer.fileNesting.enabled`, a file nested under another is left out and listed, sorted, in its parent's
    /// `nested` (explorerModel.ts `fetchChildren`). Names are matched lowercased, as `getPlatformAwareName` on a disk
    /// without PathCaseSensitive.
    fn entries(&self, folder: &Path) -> Vec<Entry> {
        let children = read_children(folder);
        let sorter = self.file_sorter();
        let visible = |name: &str| !EXCLUDED_NAMES.contains(&name);
        // `files.exclude` filters the tree after nesting, so an excluded file still nests the files under it.
        let nests = self.nester(folder).map(|nester| {
            let files: Vec<String> = children.iter().filter(|child| !child.is_dir).map(|child| child.name.to_lowercase()).collect();
            let dirname = folder.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
            nester.nest(&files, &dirname)
        });
        let mut entries = Vec::new();
        for child in &children {
            if !visible(&child.name) {
                continue;
            }
            let mut nested = Vec::new();
            if let Some(nests) = &nests
                && !child.is_dir
            {
                let Some(nested_names) = nests.get(&child.name.to_lowercase()) else { continue };
                nested = children.iter().filter(|other| !other.is_dir && visible(&other.name) && nested_names.contains(&other.name.to_lowercase())).cloned().collect();
                nested.sort_by(|a, b| sorter.compare(a, false, b, false));
            }
            entries.push(Entry { child: child.clone(), nested, is_new: false });
        }
        if let Some(item) = self.new_item.as_ref().filter(|item| item.parent == folder) {
            let child = Child { path: folder.to_path_buf(), name: String::new(), is_dir: item.is_dir, mtime: SystemTime::now() };
            entries.push(Entry { child, nested: Vec::new(), is_new: true });
        }
        entries.sort_by(|a, b| sorter.compare(&a.child, !a.nested.is_empty(), &b.child, !b.nested.is_empty()));
        entries
    }

    fn file_sorter(&self) -> FileSorter {
        let settings = self.settings.lock().unwrap();
        let setting = |key| settings.get_value(Some(key), &Overrides::default()).unwrap_or(Value::Null);
        FileSorter {
            order: setting("explorer.sortOrder").as_str().unwrap_or("default").to_string(),
            lexicographic: setting("explorer.sortOrderLexicographicOptions").as_str().unwrap_or("default").to_string(),
            reverse: setting("explorer.sortOrderReverse") == json!(true),
        }
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

/// An item of a folder, with the files nested under it; `is_new` marks the item being created, with an empty name.
struct Entry {
    child: Child,
    nested: Vec<Child>,
    is_new: bool,
}

#[derive(Clone)]
struct Child {
    path: PathBuf,
    name: String,
    is_dir: bool,
    mtime: SystemTime,
}

/// The items of `folder` in no order, excluded names too.
fn read_children(folder: &Path) -> Vec<Child> {
    // shortcut: an unreadable folder shows no children; the reference shows the error in the tree.
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let metadata = std::fs::metadata(&path).ok();
        children.push(Child {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: metadata.as_ref().is_some_and(|metadata| metadata.is_dir()),
            mtime: metadata.and_then(|metadata| metadata.modified().ok()).unwrap_or(UNIX_EPOCH),
            path,
        });
    }
    children
}

/// explorerViewer.ts `FileSorter` with `explorer.sortOrder`, `explorer.sortOrderLexicographicOptions` and
/// `explorer.sortOrderReverse`.
struct FileSorter {
    order: String,
    lexicographic: String,
    reverse: bool,
}

impl FileSorter {
    fn compare(&self, a: &Child, a_nests: bool, b: &Child, b_nests: bool) -> Ordering {
        let ((a, a_nests), (b, b_nests)) = match self.reverse {
            true => ((b, b_nests), (a, a_nests)),
            false => ((a, a_nests), (b, b_nests)),
        };
        let folders_first = b.is_dir.cmp(&a.is_dir);
        match self.order.as_str() {
            "type" if a.is_dir && b.is_dir => return self.compare_names(&a.name, &b.name),
            "filesFirst" if a.is_dir != b.is_dir => return folders_first.reverse(),
            "foldersNestsFiles" if a.is_dir != b.is_dir => return folders_first,
            "foldersNestsFiles" if a_nests != b_nests => return b_nests.cmp(&a_nests),
            "mixed" | "filesFirst" | "foldersNestsFiles" => {}
            _ if a.is_dir != b.is_dir => return folders_first,
            _ => {}
        }
        match self.order.as_str() {
            "type" => self.compare_extensions(&a.name, &b.name),
            "modified" => b.mtime.cmp(&a.mtime).then_with(|| self.compare_names(&a.name, &b.name)),
            _ => self.compare_names(&a.name, &b.name),
        }
    }

    /// comparers.ts `compareFileNames{Default,Upper,Lower,Unicode}`.
    fn compare_names(&self, one: &str, other: &str) -> Ordering {
        match self.lexicographic.as_str() {
            "upper" => compare_case_upper_first(one, other).then_with(|| compare_file_names(one, other)),
            "lower" => compare_case_upper_first(one, other).reverse().then_with(|| compare_file_names(one, other)),
            "unicode" => one.encode_utf16().cmp(other.encode_utf16()),
            _ => compare_file_names(one, other),
        }
    }

    /// comparers.ts `compareFileExtensions{Default,Upper,Lower,Unicode}`: the extensions without case, then the names.
    fn compare_extensions(&self, one: &str, other: &str) -> Ordering {
        let (one_extension, other_extension) = (extension(one), extension(other));
        if self.lexicographic == "unicode" {
            return one_extension.to_lowercase().encode_utf16().cmp(other_extension.to_lowercase().encode_utf16()).then_with(|| one.encode_utf16().cmp(other.encode_utf16()));
        }
        static CASE_INSENSITIVE: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
            let mut options = CollatorOptions::default();
            options.strength = Some(Strength::Secondary);
            Collator::try_new(numeric_preferences(), options).expect("the root collation is compiled in")
        });
        let by_extension = CASE_INSENSITIVE.compare(one_extension, other_extension).then_with(|| utf16_len(one_extension).cmp(&utf16_len(other_extension)));
        let by_case = match self.lexicographic.as_str() {
            "upper" => compare_case_upper_first(one, other),
            "lower" => compare_case_upper_first(one, other).reverse(),
            _ => Ordering::Equal,
        };
        by_extension.then(by_case).then_with(|| compare_file_names(one, other))
    }
}

/// comparers.ts `extractExtension`: after the last dot, none for a dotfile or a name without one.
fn extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !stem.starts_with('.') => extension,
        _ => "",
    }
}

/// comparers.ts `compareCaseUpperFirst`: a name starting with an upper case letter before one starting with a lower
/// case letter; `compareCaseLowerFirst` is its reverse.
fn compare_case_upper_first(one: &str, other: &str) -> Ordering {
    let starts_with_lower = |name: &str| name.chars().next().is_some_and(|first| first.to_uppercase().ne([first]));
    let starts_with_upper = |name: &str| name.chars().next().is_some_and(|first| first.to_lowercase().ne([first]));
    if starts_with_upper(one) && starts_with_lower(other) {
        return Ordering::Less;
    }
    if starts_with_lower(one) && starts_with_upper(other) {
        return Ordering::Greater;
    }
    Ordering::Equal
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn numeric_preferences() -> CollatorPreferences {
    let mut preferences = CollatorPreferences::default();
    preferences.numeric_ordering = Some(CollationNumericOrdering::True);
    preferences
}

/// comparers.ts `compareFileNamesDefault`: `Intl.Collator(undefined, { numeric: true })`, then the shorter name
/// first, since numeric collation makes `foo1` and `foo01` equal.
pub fn compare_file_names(one: &str, other: &str) -> Ordering {
    static COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| Collator::try_new(numeric_preferences(), CollatorOptions::default()).expect("the root collation is compiled in"));
    COLLATOR.compare(one, other).then_with(|| utf16_len(one).cmp(&utf16_len(other)))
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
        assert_eq!(tree.context(true, false), [root.join("Docs"), root.join("src/main.ts")], "a multi-selection wins over the focused row");
        tree.click_select(&root.join("Docs"), true, false);
        tree.click_select(&root.join("Docs"), true, false);
        assert_eq!(tree.context(true, false), [root.join("src/main.ts"), root.join("Docs")]);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// explorerView.test.ts `getContext`.
    #[test]
    fn get_context_matches_upstream() {
        let [s1, s2, s3, s4] = ["/", "/path", "/path/to", "/path/to/stat"].map(PathBuf::from);
        let none = |_: &Path| None;
        assert_eq!(get_context(Some(&s1), &[s2.clone(), s3.clone(), s4.clone()], true, none), [s2.clone(), s3.clone(), s4.clone()]);
        assert_eq!(get_context(Some(&s1), &[s1.clone(), s3.clone(), s4.clone()], true, none), [s1.clone(), s3.clone(), s4.clone()]);
        assert_eq!(get_context(Some(&s1), &[s3.clone(), s1.clone(), s4.clone()], false, none), [s1.clone()]);
        assert_eq!(get_context(None, &[s3.clone(), s1.clone(), s4.clone()], false, none), Vec::<PathBuf>::new());
        assert_eq!(get_context(None, &[s3.clone(), s1.clone(), s4.clone()], true, none), [s3.clone(), s1.clone(), s4.clone()]);

        // "keeps the compressed-folder navigation target when replacing the selection": a compact row focused at its
        // first folder acts on that folder; one only selected stands for all its folders.
        let compact = |path: &Path| (path == s3).then(|| Navigation { items: vec![s2.clone(), s3.clone()], index: 0 });
        assert_eq!(get_context(Some(&s3), &[s3.clone()], true, compact), [s2.clone()]);
        assert_eq!(get_context(Some(&s4), &[s3.clone(), s4.clone()], true, compact), [s2.clone(), s3.clone(), s4.clone()]);
    }

    /// explorerView.test.ts `compressed navigation controller`.
    #[test]
    fn compressed_navigation_moves_within_its_folders() {
        let [s1, s2, s3] = ["/path", "/path/to", "/path/to/stat"].map(PathBuf::from);
        let mut navigation = Navigation { items: vec![s1.clone(), s2.clone(), s3.clone()], index: 2 };
        navigation.next();
        assert_eq!(navigation.current(), &s3);
        navigation.previous();
        assert_eq!(navigation.current(), &s2);
        navigation.previous();
        navigation.previous();
        assert_eq!(navigation.current(), &s1);
        navigation.last();
        assert_eq!(navigation.current(), &s3);
        navigation.first();
        assert_eq!(navigation.current(), &s1);
        navigation.set_index(1);
        assert_eq!(navigation.current(), &s2);
        navigation.set_index(44);
        assert_eq!(navigation.current(), &s2);
    }

    #[test]
    fn compacts_single_child_folders_into_one_row() {
        let root = std::env::temp_dir().join(format!("explorer-compact-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("a/b/c/deep.txt"), "").unwrap();
        std::fs::write(root.join("lib/one.ts"), "").unwrap();
        let user = root.with_extension("user");
        std::fs::create_dir_all(&user).unwrap();
        let settings = Arc::new(Mutex::new(Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None)));
        let mut tree = FileTree::new(Workspace::folder(root.clone()), settings.clone());
        let row = |name: &str, depth| (name.to_string(), depth);
        assert_eq!(names(&tree), [row("a", 1), row("lib", 1)], "a folder never expanded shows alone");

        tree.click_select(&root.join("a"), false, false);
        tree.toggle(&root.join("a"));
        assert_eq!(names(&tree), [row("c", 1), row("deep.txt", 2), row("lib", 1)], "expanding opens the single-child chain");
        assert_eq!((&tree.selected, &tree.focused), (&vec![root.join("a/b/c")], &Some(root.join("a/b/c"))), "the clicked folder's row stays selected");
        let rows = tree.rows();
        let folders: Vec<&str> = rows[0].compressed.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!((folders, &rows[0].path), (vec!["a", "b", "c"], &root.join("a/b/c")));

        let mut navigation = tree.navigation(&rows[0]).unwrap();
        assert_eq!(navigation.current(), &root.join("a/b/c"));
        navigation.first();
        tree.set_navigation(&navigation);
        tree.click_select(&root.join("a/b/c"), false, false);
        assert_eq!(tree.context(true, false), [root.join("a")], "commands act on the compact row's current folder");

        tree.toggle(&root.join("a/b/c"));
        assert_eq!(names(&tree), [row("c", 1), row("lib", 1)], "a collapsed compact row stays compact");
        let stored = tree.view_state(0.)["expanded"].to_string();
        assert!(!stored.contains("/a\"") && !stored.contains("/a/b"), "nor does it store its folders as expanded: {stored}");
        tree.toggle(&root.join("a/b/c"));

        tree.new_item = Some(NewItem { parent: root.join("a/b"), is_dir: false });
        let rows = tree.rows();
        assert_eq!(rows[0].compressed.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>(), ["a", "b"], "a new item splits the row");
        tree.new_item = None;

        std::fs::write(root.join("a/b/side.txt"), "").unwrap();
        assert_eq!(names(&tree)[..3], [row("b", 1), row("c", 2), row("deep.txt", 3)], "a second child ends the chain");

        settings.lock().unwrap().update_value_derived("explorer.compactFolders", Some(&json!(false))).unwrap();
        assert_eq!(names(&tree)[..2], [row("a", 1), row("b", 2)]);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(user).unwrap();
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

    /// Cases from comparers.test.ts for the Upper, Lower, Unicode and Extensions comparers.
    #[test]
    fn sorts_names_with_each_lexicographic_option() {
        let sorter = |lexicographic: &str| FileSorter { order: "default".into(), lexicographic: lexicographic.into(), reverse: false };
        let sorted = |lexicographic: &str, extensions: bool, names: &[&'static str]| {
            let sorter = sorter(lexicographic);
            let mut names = names.to_vec();
            names.sort_by(|a, b| if extensions { sorter.compare_extensions(a, b) } else { sorter.compare_names(a, b) });
            names
        };
        let numbers = ["a10.txt", "A2.txt", "A100.txt", "a20.txt"];
        let roots = ["artichoke", "Artichoke", "art", "Art"];
        let emails = ["email", "Email", "émail", "Émail"];
        for extensions in [false, true] {
            assert_eq!(sorted("upper", extensions, &roots), ["Art", "Artichoke", "art", "artichoke"]);
            assert_eq!(sorted("upper", extensions, &emails), ["Email", "Émail", "email", "émail"]);
            assert_eq!(sorted("upper", extensions, &numbers), ["A2.txt", "A100.txt", "a10.txt", "a20.txt"]);
            assert_eq!(sorted("lower", extensions, &roots), ["art", "artichoke", "Art", "Artichoke"]);
            assert_eq!(sorted("lower", extensions, &emails), ["email", "émail", "Email", "Émail"]);
            assert_eq!(sorted("lower", extensions, &numbers), ["a10.txt", "a20.txt", "A2.txt", "A100.txt"]);
            assert_eq!(sorted("default", extensions, &numbers), ["A2.txt", "a10.txt", "a20.txt", "A100.txt"]);
        }
        assert_eq!(sorted("unicode", false, &roots), ["Art", "Artichoke", "art", "artichoke"]);
        assert_eq!(sorted("unicode", false, &emails), ["Email", "email", "Émail", "émail"]);
        assert_eq!(sorted("unicode", false, &numbers), ["A100.txt", "A2.txt", "a10.txt", "a20.txt"]);
        assert_eq!(sorted("unicode", true, &numbers), ["A100.txt", "A2.txt", "a10.txt", "a20.txt"]);

        let default = sorter("default");
        assert_eq!(default.compare_extensions("bbb.aaa", "aaa.bbb"), Ordering::Less, "the extension first");
        assert_eq!(default.compare_extensions("a.MD", "a.md"), compare_file_names("a.MD", "a.md"), "extensions equal but for case compare as full names");
        assert_eq!(default.compare_extensions(".md", ".Gitattributes"), Ordering::Greater, "a dotfile has no extension");
        assert_eq!(default.compare_extensions("txt.abc1", "txt.abc10"), Ordering::Less, "extensions compare numerically");
        assert_eq!(default.compare_extensions("a.ext1", "b.Ext1"), Ordering::Less);
        assert_eq!(sorter("unicode").compare_extensions("a.MD", "b.md"), Ordering::Less, "unicode extensions compare without case");
        assert_eq!(default.compare_names("bbb.aaa", "aaa.bbb"), Ordering::Greater);
    }

    #[test]
    fn sorts_items_with_each_sort_order() {
        let root = std::env::temp_dir().join(format!("explorer-sort-order-test-{}", std::process::id()));
        let now = SystemTime::now();
        std::fs::create_dir_all(&root).unwrap();
        for (age, file) in [(3, "a.ts"), (1, "a.js"), (2, "c.md"), (4, "z.css"), (5, "0.md"), (0, "b/"), (5, "x/")] {
            let file = match file.strip_suffix('/') {
                Some(folder) => std::fs::create_dir_all(root.join(folder)).and_then(|_| std::fs::File::open(root.join(folder))),
                None => std::fs::File::create(root.join(file)),
            };
            file.unwrap().set_modified(now - std::time::Duration::from_secs(age * 60)).unwrap();
        }
        let user = root.with_extension("user");
        std::fs::create_dir_all(&user).unwrap();
        let settings = Arc::new(Mutex::new(Settings::load(config::registry::Registry::with_extensions(&[]), user.clone(), None)));
        let tree = FileTree::new(Workspace::folder(root.clone()), settings.clone());
        let sorted_with = |order: &str, reverse: bool| {
            let mut settings = settings.lock().unwrap();
            settings.update_value_derived("explorer.sortOrder", Some(&json!(order))).unwrap();
            settings.update_value_derived("explorer.sortOrderReverse", Some(&json!(reverse))).unwrap();
            drop(settings);
            names(&tree).into_iter().map(|(name, _)| name).collect::<Vec<_>>()
        };
        assert_eq!(sorted_with("default", false), ["b", "x", "0.md", "a.js", "a.ts", "c.md", "z.css"]);
        assert_eq!(sorted_with("default", true), ["z.css", "c.md", "a.ts", "a.js", "0.md", "x", "b"], "reversed, files come first");
        assert_eq!(sorted_with("mixed", false), ["0.md", "a.js", "a.ts", "b", "c.md", "x", "z.css"]);
        assert_eq!(sorted_with("filesFirst", false), ["0.md", "a.js", "a.ts", "c.md", "z.css", "b", "x"]);
        assert_eq!(sorted_with("type", false), ["b", "x", "z.css", "a.js", "0.md", "c.md", "a.ts"]);
        assert_eq!(sorted_with("modified", false), ["b", "x", "a.js", "c.md", "a.ts", "z.css", "0.md"]);

        settings.lock().unwrap().update_value_derived("explorer.fileNesting.enabled", Some(&json!(true))).unwrap();
        assert_eq!(sorted_with("default", false), ["b", "x", "0.md", "a.ts", "a.js", "c.md", "z.css"]);
        assert_eq!(sorted_with("foldersNestsFiles", false), ["b", "x", "a.ts", "a.js", "0.md", "c.md", "z.css"], "a.ts nests a.js and comes before the files without nests");
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(user).unwrap();
    }
}
