//! scmViewPane.ts view modes: the list sorted by `SCMTreeSorter` (path, name or status), and the tree of
//! `ResourceTree` folders, compressed with `scm.compactFolders`, folders before files. The mode is `scm.viewMode` in
//! workspace storage, `scm.defaultViewMode` when unset.

use std::path::{Path, PathBuf};

use gpui::{ClickEvent, Context, Div, Stateful, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::scm_view::{INDENT, ROW_HEIGHT, TWISTIE};
use crate::commands::Outcome;
use crate::explorer::compare_file_names;
use crate::file_icons::FolderIcon;
use crate::scm::{Group, Provider, Resource};
use crate::search_model::compare_paths;

const VIEW_MODE_KEY: &str = "scm.viewMode";
const VIEW_SORT_KEY: &str = "scm.viewSortKey";

/// One row below a group: a folder (path relative to the root, its label with compressed parents joined by `/`) or a
/// resource, with its tree depth.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeRow {
    Folder { path: PathBuf, label: String, depth: usize },
    Resource { index: usize, depth: usize },
}

impl Workbench {
    pub(super) fn register_scm_view_mode_commands(&mut self) {
        for (ids, mode) in [(["workbench.scm.action.setListViewMode", "workbench.scm.action.setListViewModeNavigation"], "list"), (["workbench.scm.action.setTreeViewMode", "workbench.scm.action.setTreeViewModeNavigation"], "tree")] {
            for id in ids {
                self.commands.register(id, move |workbench, _, _, cx| {
                    workbench.workspace_storage.lock().unwrap().set(VIEW_MODE_KEY, mode.to_string());
                    cx.notify();
                    Outcome::done()
                });
            }
        }
        for (id, key) in [("workbench.scm.action.setSortKey.name", "name"), ("workbench.scm.action.setSortKey.path", "path"), ("workbench.scm.action.setSortKey.status", "status")] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.workspace_storage.lock().unwrap().set(VIEW_SORT_KEY, key.to_string());
                cx.notify();
                Outcome::done()
            });
        }
    }

    /// `ResourceRenderer` of a folder node: twistie, folder icon and the compressed label; a click toggles it.
    pub(super) fn scm_folder_row(&self, provider: &Provider, group: &Group, path: PathBuf, label: String, depth: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let key = (provider.handle, group.handle, path);
        let expanded = !self.scm.collapsed_folders.contains(&key);
        let name = label.rsplit('/').next().unwrap_or_default().to_string();
        let icon = self.file_icons.as_ref().and_then(|icons| icons.folder_icon(FolderIcon { name: &name, is_root: false, expanded }, self.theme.kind, 13., ROW_HEIGHT, foreground)).unwrap_or_else(|| div().w(px(16.)));
        let id = gpui::SharedString::from(format!("scm-folder-{}-{}-{}", provider.handle, group.handle, key.2.display()));
        div()
            .id(id)
            .h(px(ROW_HEIGHT))
            .pl(px(INDENT * (1 + depth) as f32))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                if !workbench.scm.collapsed_folders.remove(&key) {
                    workbench.scm.collapsed_folders.insert(key.clone());
                }
                cx.notify();
            }))
            .child(self.icon(if expanded { "chevron-down" } else { "chevron-right" }, TWISTIE, 16., foreground))
            .child(icon.mr(px(6.)))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
    }

    pub(super) fn scm_tree_mode(&self) -> bool {
        let stored = self.workspace_storage.lock().unwrap().get(VIEW_MODE_KEY).map(str::to_string);
        stored.map_or(self.setting("scm.defaultViewMode") == json!("tree"), |mode| mode == "tree")
    }

    /// `scm.viewSortKey` in workspace storage, `scm.defaultViewSortKey` when unset.
    fn scm_sort_key(&self) -> String {
        let stored = self.workspace_storage.lock().unwrap().get(VIEW_SORT_KEY).map(str::to_string);
        stored.unwrap_or_else(|| self.setting("scm.defaultViewSortKey").as_str().unwrap_or("path").to_string())
    }

    /// The rows of one group's resources in the current view mode; folders in `collapsed` hide their children.
    pub(super) fn scm_rows(&self, root: &Path, resources: &[Resource], collapsed: &dyn Fn(&Path) -> bool) -> Vec<TreeRow> {
        if !self.scm_tree_mode() {
            return list_rows(resources, &self.scm_sort_key());
        }
        tree_rows(root, resources, self.setting("scm.compactFolders") != json!(false), collapsed)
    }
}

fn resource_path(resource: &Resource) -> PathBuf {
    PathBuf::from(resource.uri["path"].as_str().unwrap_or_default())
}

/// `SCMTreeSorter` in list mode.
pub fn list_rows(resources: &[Resource], sort_key: &str) -> Vec<TreeRow> {
    let mut order: Vec<usize> = (0..resources.len()).collect();
    order.sort_by(|&a, &b| {
        let (one, other) = (&resources[a], &resources[b]);
        let name = |resource: &Resource| resource_path(resource).file_name().unwrap_or_default().to_string_lossy().into_owned();
        if sort_key == "name" {
            return compare_file_names(&name(one), &name(other));
        }
        if sort_key == "status" && one.tooltip != other.tooltip {
            return one.tooltip.cmp(&other.tooltip);
        }
        compare_paths(&resource_path(one).to_string_lossy(), &resource_path(other).to_string_lossy())
    });
    order.into_iter().map(|index| TreeRow::Resource { index, depth: 0 }).collect()
}

/// A `ResourceTree` folder: its child folders by name and its resources.
#[derive(Default)]
struct Folder {
    folders: Vec<(String, Folder)>,
    resources: Vec<usize>,
}

/// `ResourceTree` of the resources under `root`, then the rows of a compressible tree, folders first by name.
pub fn tree_rows(root: &Path, resources: &[Resource], compact: bool, collapsed: &dyn Fn(&Path) -> bool) -> Vec<TreeRow> {
    let mut top = Folder::default();
    for (index, resource) in resources.iter().enumerate() {
        let path = resource_path(resource);
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let mut folder = &mut top;
        let parts: Vec<String> = relative.parent().into_iter().flat_map(|parent| parent.components()).map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
        for part in parts {
            let position = match folder.folders.iter().position(|(name, _)| *name == part) {
                Some(position) => position,
                None => {
                    folder.folders.push((part, Folder::default()));
                    folder.folders.len() - 1
                }
            };
            folder = &mut folder.folders[position].1;
        }
        folder.resources.push(index);
    }
    let mut rows = Vec::new();
    push_children(&top, PathBuf::new(), 0, resources, compact, collapsed, &mut rows);
    rows
}

fn push_children(folder: &Folder, path: PathBuf, depth: usize, resources: &[Resource], compact: bool, collapsed: &dyn Fn(&Path) -> bool, rows: &mut Vec<TreeRow>) {
    let mut folders: Vec<&(String, Folder)> = folder.folders.iter().collect();
    folders.sort_by(|a, b| compare_file_names(&a.0, &b.0));
    for (name, child) in folders {
        let mut label = name.clone();
        let mut child_path = path.join(name);
        let mut node = child;
        // compressedObjectTreeModel.ts: a folder whose only child is a folder joins it.
        while compact && node.resources.is_empty() && node.folders.len() == 1 {
            let (next_name, next) = &node.folders[0];
            label = format!("{label}/{next_name}");
            child_path = child_path.join(next_name);
            node = next;
        }
        rows.push(TreeRow::Folder { path: child_path.clone(), label, depth });
        if !collapsed(&child_path) {
            push_children(node, child_path, depth + 1, resources, compact, collapsed, rows);
        }
    }
    let mut files = folder.resources.clone();
    files.sort_by(|&a, &b| {
        let name = |index: usize| resource_path(&resources[index]).file_name().unwrap_or_default().to_string_lossy().into_owned();
        compare_file_names(&name(a), &name(b))
    });
    rows.extend(files.into_iter().map(|index| TreeRow::Resource { index, depth }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resources(paths: &[&str]) -> Vec<Resource> {
        paths.iter().map(|path| Resource { uri: json!({ "scheme": "file", "path": format!("/r/{path}") }), ..Resource::default() }).collect()
    }

    #[test]
    fn the_tree_puts_folders_first_and_compresses_single_folders() {
        let resources = resources(&["b.txt", "src/deep/x.ts", "a.txt", "lib/one.ts", "lib/sub/two.ts"]);
        let rows = tree_rows(Path::new("/r"), &resources, true, &|_| false);
        assert_eq!(
            rows,
            vec![
                TreeRow::Folder { path: "lib".into(), label: "lib".into(), depth: 0 },
                TreeRow::Folder { path: "lib/sub".into(), label: "sub".into(), depth: 1 },
                TreeRow::Resource { index: 4, depth: 2 },
                TreeRow::Resource { index: 3, depth: 1 },
                TreeRow::Folder { path: "src/deep".into(), label: "src/deep".into(), depth: 0 },
                TreeRow::Resource { index: 1, depth: 1 },
                TreeRow::Resource { index: 2, depth: 0 },
                TreeRow::Resource { index: 0, depth: 0 },
            ]
        );
        let collapsed = tree_rows(Path::new("/r"), &resources, true, &|path| path == Path::new("lib"));
        assert_eq!(collapsed.len(), 5);
    }

    #[test]
    fn the_list_sorts_by_path_then_by_name() {
        let resources = resources(&["z/a.txt", "b.txt", "a/c.txt"]);
        let order = |rows: Vec<TreeRow>| rows.into_iter().map(|row| match row { TreeRow::Resource { index, .. } => index, TreeRow::Folder { .. } => usize::MAX }).collect::<Vec<_>>();
        assert_eq!(order(list_rows(&resources, "path")), [1, 2, 0]);
        assert_eq!(order(list_rows(&resources, "name")), [0, 1, 2]);
    }
}
