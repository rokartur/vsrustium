//! An extension's tree view on the main thread (treeView.ts `TreeView`, mainThreadTreeViews.ts
//! `TreeViewDataProvider`): the items the host sent by handle, the children fetched per parent, and the
//! expanded, selected and focused items.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// views.ts `TreeItemCollapsibleState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Collapsible {
    None,
    Collapsed,
    Expanded,
}

#[derive(Clone, Debug)]
pub struct TreeItem {
    pub handle: String,
    pub parent: Option<String>,
    pub collapsible: Collapsible,
    /// The host's `ITreeItem`, for the label, description, icon, tooltip, command and context value.
    pub dto: Value,
}

impl TreeItem {
    pub fn from_dto(dto: Value) -> TreeItem {
        let collapsible = match dto["collapsibleState"].as_u64() {
            Some(1) => Collapsible::Collapsed,
            Some(2) => Collapsible::Expanded,
            _ => Collapsible::None,
        };
        TreeItem { handle: dto["handle"].as_str().unwrap_or_default().to_owned(), parent: dto["parentHandle"].as_str().map(str::to_owned), collapsible, dto }
    }

    /// treeView.ts `TreeRenderer.renderElement`: the label, else the resource's base name.
    pub fn label(&self) -> String {
        if let Some(label) = self.dto["label"]["label"].as_str() {
            // treeView.ts: a markdown label `~~text~~` is drawn struck through.
            return label.strip_prefix("~~").and_then(|label| label.strip_suffix("~~")).unwrap_or(label).to_owned();
        }
        let path = self.dto["resourceUri"]["path"].as_str().unwrap_or_default();
        path.rsplit('/').next().unwrap_or_default().to_owned()
    }

    pub fn strikethrough(&self) -> bool {
        self.dto["label"]["label"].as_str().is_some_and(|label| label.len() > 4 && label.starts_with("~~") && label.ends_with("~~"))
    }

    pub fn description(&self) -> Option<String> {
        match &self.dto["description"] {
            Value::String(description) => Some(description.clone()),
            // `description: true` shows the resource's folder (treeView.ts `resourceLabel` with `hidePath: false`).
            Value::Bool(true) => {
                let path = self.dto["resourceUri"]["path"].as_str()?;
                Some(path.rsplit_once('/').map(|(folder, _)| folder.to_owned()).unwrap_or_default())
            }
            _ => None,
        }
    }

    pub fn theme_icon(&self) -> Option<&str> {
        self.dto["themeIcon"]["id"].as_str()
    }
}

/// One view's tree. `roots` is `None` until the first fetch (`TreeViewDataProvider.isEmpty`).
#[derive(Default)]
pub struct CustomTree {
    pub items: HashMap<String, TreeItem>,
    pub roots: Option<Vec<String>>,
    pub children: HashMap<String, Vec<String>>,
    pub expanded: HashSet<String>,
    pub selection: Vec<String>,
    pub focus: Option<String>,
    pub message: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub badge: Option<Value>,
    pub has_provider: bool,
    pub can_select_many: bool,
    pub show_collapse_all: bool,
    /// Parents whose children are being fetched, so a redraw does not ask again.
    pub fetching: HashSet<Option<String>>,
}

impl CustomTree {
    /// `TreeViewDataProvider.getChildrenBatch` without a parent clears the items, then `postGetChildren` keeps the
    /// new ones; an item the host sent as Expanded starts expanded.
    pub fn set_children(&mut self, parent: Option<&str>, dtos: Vec<Value>) {
        if parent.is_none() {
            self.items.clear();
            self.children.clear();
        }
        let mut handles = Vec::new();
        for dto in dtos {
            let item = TreeItem::from_dto(dto);
            if item.collapsible == Collapsible::Expanded && !self.items.contains_key(&item.handle) {
                self.expanded.insert(item.handle.clone());
            }
            handles.push(item.handle.clone());
            self.items.insert(item.handle.clone(), item);
        }
        match parent {
            Some(parent) => drop(self.children.insert(parent.to_owned(), handles)),
            None => self.roots = Some(handles),
        }
    }

    /// `getItemsToRefresh` and `updateTreeItem`: known items take the new properties; the parents whose children
    /// changed (an item's handle changed, or the whole tree for `None`) are returned to fetch again.
    pub fn refresh(&mut self, items: &serde_json::Map<String, Value>) -> Vec<Option<String>> {
        let mut refetch = Vec::new();
        for (old_handle, dto) in items {
            if !self.items.contains_key(old_handle) {
                continue;
            }
            let item = TreeItem::from_dto(dto.clone());
            if item.handle == *old_handle {
                self.items.insert(item.handle.clone(), item);
                continue;
            }
            self.items.remove(old_handle);
            refetch.push(item.parent.clone());
            self.items.insert(item.handle.clone(), item);
        }
        refetch
    }

    /// The rows the tree draws: each visible item with its depth, children under an expanded parent once fetched.
    pub fn rows(&self) -> Vec<(&TreeItem, usize)> {
        let mut rows = Vec::new();
        if let Some(roots) = &self.roots {
            self.push_rows(roots, 0, &mut rows);
        }
        rows
    }

    fn push_rows<'a>(&'a self, handles: &[String], depth: usize, rows: &mut Vec<(&'a TreeItem, usize)>) {
        for handle in handles {
            let Some(item) = self.items.get(handle) else { continue };
            rows.push((item, depth));
            if item.collapsible != Collapsible::None && self.expanded.contains(handle) && let Some(children) = self.children.get(handle) {
                self.push_rows(children, depth + 1, rows);
            }
        }
    }

    /// Expanded items without fetched children: the ones `rows` needs next.
    pub fn missing_children(&self) -> Vec<String> {
        let mut missing = Vec::new();
        for (item, _) in self.rows() {
            if item.collapsible != Collapsible::None && self.expanded.contains(&item.handle) && !self.children.contains_key(&item.handle) {
                missing.push(item.handle.clone());
            }
        }
        missing
    }

    /// The parents of `handle` from the root down, by `parentHandle`.
    pub fn parent_chain(&self, handle: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = self.items.get(handle).and_then(|item| item.parent.clone());
        while let Some(parent) = current {
            current = self.items.get(&parent).and_then(|item| item.parent.clone());
            chain.insert(0, parent);
        }
        chain
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn item(handle: &str, parent: Option<&str>, state: u64) -> Value {
        json!({ "handle": handle, "parentHandle": parent, "label": { "label": handle }, "collapsibleState": state })
    }

    #[test]
    fn rows_show_fetched_children_of_expanded_items_only() {
        let mut tree = CustomTree::default();
        tree.set_children(None, vec![item("a", None, 1), item("b", None, 2), item("c", None, 0)]);
        assert_eq!(tree.missing_children(), vec!["b".to_string()]);
        tree.set_children(Some("b"), vec![item("b/1", Some("b"), 0)]);
        tree.set_children(Some("a"), vec![item("a/1", Some("a"), 0)]);
        let rows: Vec<_> = tree.rows().into_iter().map(|(item, depth)| (item.handle.clone(), depth)).collect();
        assert_eq!(rows, vec![("a".into(), 0), ("b".into(), 0), ("b/1".into(), 1), ("c".into(), 0)]);
        assert_eq!(tree.parent_chain("b/1"), vec!["b".to_string()]);
    }

    #[test]
    fn refresh_updates_known_items_and_refetches_the_parent_of_a_changed_handle() {
        let mut tree = CustomTree::default();
        tree.set_children(None, vec![item("a", None, 1)]);
        tree.set_children(Some("a"), vec![item("a/1", Some("a"), 0)]);
        let mut renamed = item("a/2", Some("a"), 0);
        renamed["label"] = json!({ "label": "~~two~~" });
        let changes = json!({ "a": { "handle": "a", "label": { "label": "A" }, "collapsibleState": 1 }, "a/1": renamed, "x": item("x", None, 0) });
        assert_eq!(tree.refresh(changes.as_object().unwrap()), vec![Some("a".to_string())]);
        assert_eq!(tree.items["a"].label(), "A");
        assert_eq!((tree.items["a/2"].label(), tree.items["a/2"].strikethrough()), ("two".to_string(), true));
        assert!(!tree.items.contains_key("x"));
    }
}
