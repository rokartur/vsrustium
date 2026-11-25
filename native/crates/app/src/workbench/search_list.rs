//! listCommands.ts and listWidget.ts `MouseController` on the Search results tree beyond moving the focus: multiple
//! selection (`multipleSelectionSupport: true`) from the mouse and the keyboard, the focus moves that ignore
//! filters, the parent, and collapsing to the focus.

use std::collections::HashMap;

use gpui::{Context, Modifiers, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::search_view::{ResultRow, RowKey, RowKind};
use crate::list_tree::parent;

impl Workbench {
    /// listService.ts `WorkbenchListSupportsMultiSelectContextKey`, `WorkbenchListHasSelectionOrFocus` and the tree's
    /// `WorkbenchTreeElement*` keys of the focused row, while the results have the focus.
    pub(super) fn search_list_context(&self, context: &mut HashMap<String, Value>) {
        context.insert("listSupportsMultiselect".into(), json!(true));
        context.insert("listHasSelectionOrFocus".into(), json!(self.search.focused.is_some() || !self.search.selected.is_empty()));
        context.insert("listDoubleSelection".into(), json!(self.search.selected.len() == 2));
        context.insert("listMultiSelection".into(), json!(self.search.selected.len() > 1));
        let rows = self.search_rows();
        let Some(index) = rows.iter().position(|row| Some(&row.key) == self.search.focused.as_ref()) else { return };
        let collapsed = match rows[index].kind {
            RowKind::Folder { collapsed, .. } | RowKind::File { collapsed, .. } => Some(collapsed),
            RowKind::Match { .. } => None,
        };
        context.insert("treeElementCanCollapse".into(), json!(collapsed == Some(false)));
        context.insert("treeElementCanExpand".into(), json!(collapsed == Some(true)));
        context.insert("treeElementHasChild".into(), json!(rows.get(index + 1).is_some_and(|child| child.depth > rows[index].depth)));
        context.insert("treeElementHasParent".into(), json!(rows[index].depth > 1));
    }

    /// The row the results measure their width on with `workbench.list.horizontalScrolling`: the longest text,
    /// indent included. A uniform list measures one row; the rows' own layout decides the pixels.
    pub(super) fn widest_search_row(&self, rows: &[ResultRow]) -> usize {
        let text_length = |row: &ResultRow| match &row.kind {
            RowKind::Folder { label, .. } => label.chars().count(),
            RowKind::File { file, .. } => self.search.result.files[*file].path.as_os_str().len(),
            RowKind::Match { file, index } => self.search.result.files[*file].matches[*index].text().chars().count(),
        };
        (0..rows.len()).max_by_key(|&index| rows[index].depth * 2 + text_length(&rows[index])).unwrap_or(0)
    }

    fn search_rows(&self) -> Vec<ResultRow> {
        self.search.rows(&self.search_folders(), Some(&self.collapse_results()))
    }

    /// A click with the multi-select modifier toggles the row in the selection, with Shift selects the range from
    /// the anchor (`isSelectionChangeEvent`); returns false for a plain click, which selects and opens.
    pub(super) fn search_selection_click(&mut self, key: &RowKey, modifiers: Modifiers, cx: &mut Context<Self>) -> bool {
        let multi = if self.list_click_mode().alt_multi_select { modifiers.alt } else { modifiers.platform };
        if !multi && !modifiers.shift {
            return false;
        }
        let rows = self.search_rows();
        let clicked = rows.iter().position(|row| row.key == *key);
        let anchor = self.search.anchor.as_ref().and_then(|anchor| rows.iter().position(|row| row.key == *anchor));
        match (modifiers.shift, anchor, clicked) {
            // `changeSelection` with Shift: the anchor's range, added to the selection with the modifier.
            (true, Some(anchor), Some(clicked)) => {
                let range: Vec<RowKey> = rows[anchor.min(clicked)..=anchor.max(clicked)].iter().map(|row| row.key.clone()).collect();
                if multi {
                    let mut selection = self.search.selected.clone();
                    selection.retain(|selected| !range.contains(selected));
                    selection.extend(range);
                    self.search.selected = selection;
                } else {
                    self.search.selected = range;
                }
            }
            _ => {
                match self.search.selected.iter().position(|selected| selected == key) {
                    Some(position) => drop(self.search.selected.remove(position)),
                    None => self.search.selected.push(key.clone()),
                }
                self.search.anchor = Some(key.clone());
            }
        }
        self.search.focused = Some(key.clone());
        cx.notify();
        true
    }

    /// The list commands of the Search results the focus moves of search_actions.rs do not cover.
    pub(super) fn run_search_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.search_rows();
        if rows.is_empty() {
            return;
        }
        let focus = rows.iter().position(|row| Some(&row.key) == self.search.focused.as_ref());
        let last = rows.len() - 1;
        match id {
            "list.focusAnyDown" => self.move_search_focus(1, window, cx),
            "list.focusAnyUp" => self.move_search_focus(-1, window, cx),
            "list.focusAnyFirst" => self.move_search_focus(isize::MIN, window, cx),
            "list.focusAnyLast" => self.move_search_focus(isize::MAX, window, cx),
            "list.focusParent" => {
                let depths: Vec<usize> = rows.iter().map(|row| row.depth).collect();
                let Some(parent) = focus.and_then(|index| parent(&depths, index)) else { return };
                self.move_search_focus(parent as isize - focus.map_or(0, |index| index as isize), window, cx);
            }
            "list.expandSelectionDown" | "list.expandSelectionUp" => {
                let next = match (focus, id == "list.expandSelectionDown") {
                    (None, true) => 0,
                    (None, false) => last,
                    (Some(index), true) => (index + 1).min(last),
                    (Some(index), false) => index.saturating_sub(1),
                };
                // `expandMultiSelection`: moving back into the selection drops the row left, else the new row joins.
                if focus == Some(next) {
                    return;
                }
                let key = rows[next].key.clone();
                match self.search.selected.contains(&key) {
                    true => self.search.selected.retain(|selected| Some(selected) != focus.map(|index| &rows[index].key)),
                    false => self.search.selected.push(key.clone()),
                }
                self.search.focused = Some(key);
                self.search.scroll.scroll_to_item(next, gpui::ScrollStrategy::Top);
            }
            "list.selectAll" => self.select_all_search_rows(&rows),
            "list.toggleSelection" => {
                let Some(key) = focus.map(|index| rows[index].key.clone()) else { return };
                match self.search.selected.iter().position(|selected| *selected == key) {
                    Some(position) => drop(self.search.selected.remove(position)),
                    None => self.search.selected.push(key),
                }
            }
            // `selectElement(accessor, true)`: a folder or file row toggles, and the focus becomes the selection; a
            // match opens keeping the focus.
            "list.selectAndPreserveFocus" => {
                let Some(index) = focus else { return };
                let key = rows[index].key.clone();
                if let RowKind::Folder { collapsed, .. } | RowKind::File { collapsed, .. } = rows[index].kind {
                    self.search.collapsed.insert(key.clone(), !collapsed);
                }
                self.search.selected = vec![key.clone()];
                self.search.anchor = Some(key.clone());
                if matches!(key, RowKey::Match(..)) {
                    self.open_search_row(&key, true, false, window, cx);
                }
            }
            // abstractTree.ts `collapse(focus, true)`: the row and every folder and file under it.
            "list.collapseAllToFocus" => {
                let Some(index) = focus else { return };
                let depth = rows[index].depth;
                for row in rows[index..].iter().take_while(|row| row.key == rows[index].key || row.depth > depth) {
                    if !matches!(row.kind, RowKind::Match { .. }) {
                        self.search.collapsed.insert(row.key.clone(), true);
                    }
                }
                self.search.selected = vec![rows[index].key.clone()];
                self.search.anchor = Some(rows[index].key.clone());
            }
            // The rows of the results have no managed hover.
            "list.showHover" => {}
            _ => unreachable!("not a Search list command: {id}"),
        }
        cx.notify();
    }

    /// listCommands.ts `list.selectAll` on a tree: the shown rows under the parent of the focused row (or of the
    /// selection's first), with the parent itself when they were all selected already.
    fn select_all_search_rows(&mut self, rows: &[ResultRow]) {
        let position = |key: &RowKey| rows.iter().position(|row| row.key == *key);
        let start = match &self.search.focused {
            Some(focused) if !self.search.selected.contains(focused) => position(focused),
            _ => self.search.selected.first().and_then(position),
        };
        let depths: Vec<usize> = rows.iter().map(|row| row.depth).collect();
        let scope = start.and_then(|start| parent(&depths, start));
        let (first, depth) = match scope {
            Some(scope) => (scope + 1, rows[scope].depth),
            None => (0, 0),
        };
        let mut selection: Vec<RowKey> = rows[first..].iter().take_while(|row| row.depth > depth).map(|row| row.key.clone()).collect();
        if let Some(scope) = scope.filter(|_| self.search.selected.len() == selection.len()) {
            selection.insert(0, rows[scope].key.clone());
        }
        self.search.selected = selection;
    }
}
