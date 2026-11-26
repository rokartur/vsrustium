//! The list commands (listCommands.ts) on the Explorer tree: focus moves, expand and collapse, selection, and the
//! Explorer's own open commands on the focused row (fileCommands.ts, fileActions.ts).

use std::path::PathBuf;

use gpui::{Context, Pixels, Window, px};

use super::Workbench;
use crate::commands::Outcome;

const ROW_HEIGHT: Pixels = px(22.);

/// The list commands the Explorer takes while it has focus; the Search view's results take the same ids.
pub(super) const EXPLORER_LIST_COMMANDS: [&str; 24] = [
    "list.focusDown",
    "list.focusUp",
    "list.focusAnyDown",
    "list.focusAnyUp",
    "list.focusPageDown",
    "list.focusPageUp",
    "list.focusFirst",
    "list.focusLast",
    "list.focusAnyFirst",
    "list.focusAnyLast",
    "list.expand",
    "list.collapse",
    "list.collapseAll",
    "list.toggleExpand",
    "list.select",
    "list.expandSelectionDown",
    "list.expandSelectionUp",
    "list.selectAll",
    "list.toggleSelection",
    "list.clear",
    "list.focusParent",
    "list.collapseAllToFocus",
    "list.selectAndPreserveFocus",
    "list.showHover",
];

impl Workbench {
    pub(super) fn register_explorer_keyboard_commands(&mut self) {
        // fileCommands.ts `explorer.openAndPassFocus`: the context opened pinned, with the focus.
        self.commands.register("explorer.openAndPassFocus", |workbench, _, window, cx| {
            for path in workbench.explorer_context_files() {
                workbench.open_file(path, true, window, cx);
            }
            Outcome::done()
        });
        // fileActions.ts `openFilePreserveFocusHandler`: the context's files opened as previews, the focus kept.
        self.commands.register("filesExplorer.openFilePreserveFocus", |workbench, _, window, cx| {
            for path in workbench.explorer_context_files() {
                workbench.open_file(path, false, window, cx);
            }
            Outcome::done()
        });
        // fileCommands.ts `explorer.openToSide`: the resource a context menu passes, else the Explorer's context,
        // opened pinned in the group beside the active one (`SIDE_GROUP`).
        self.commands.register("explorer.openToSide", |workbench, args, window, cx| {
            let resources = match args.first() {
                Some(resource @ serde_json::Value::Object(_)) => vec![resource.clone()],
                _ => workbench.explorer_context_files().iter().map(|path| crate::editor::uri_value("file", path)).collect(),
            };
            for resource in resources {
                drop(workbench.execute_command("_workbench.open", vec![resource, serde_json::json!([-2, { "pinned": true, "preserveFocus": false }])], window, cx));
            }
            Outcome::done()
        });
    }

    fn explorer_context_files(&self) -> Vec<PathBuf> {
        let Some(tree) = self.folder.as_ref() else { return Vec::new() };
        tree.context(true, false).into_iter().filter(|path| !path.is_dir()).collect()
    }

    /// Runs one of `EXPLORER_LIST_COMMANDS`; `navigate` then reveals the focused row at the nearest edge.
    pub(super) fn run_explorer_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.folder.as_mut() else { return };
        let rows = tree.rows();
        if rows.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = rows.iter().map(|row| row.path.clone()).collect();
        let focus = tree.focused.as_ref().and_then(|focused| paths.iter().position(|path| path == focused));
        let last = rows.len() - 1;
        match id {
            "list.focusDown" | "list.focusAnyDown" => tree.focus(Some(paths[focus.map_or(0, |index| (index + 1).min(last))].clone())),
            "list.focusUp" | "list.focusAnyUp" => tree.focus(Some(paths[focus.map_or(last, |index| index.saturating_sub(1))].clone())),
            "list.focusFirst" | "list.focusAnyFirst" => tree.focus(Some(paths[0].clone())),
            "list.focusLast" | "list.focusAnyLast" => tree.focus(Some(paths[last].clone())),
            "list.focusPageDown" => return self.focus_explorer_page(true, focus, &paths, cx),
            "list.focusPageUp" => return self.focus_explorer_page(false, focus, &paths, cx),
            "list.expand" => {
                let Some(index) = focus else { return };
                let row = &rows[index];
                // asyncDataTree.ts `expand` answers false for an expanded row, which then moves to its first child.
                if (row.is_dir || row.has_nests) && !row.expanded {
                    tree.set_expanded(&row.path, true);
                } else if rows.get(index + 1).is_some_and(|child| child.depth > row.depth) {
                    tree.focus(Some(paths[index + 1].clone()));
                }
            }
            "list.collapse" => {
                let Some(index) = focus else { return };
                let row = &rows[index];
                if (row.is_dir || row.has_nests) && row.expanded {
                    tree.set_expanded(&row.path, false);
                } else if let Some(parent) = rows[..index].iter().rposition(|parent| parent.depth < row.depth) {
                    tree.focus(Some(paths[parent].clone()));
                }
            }
            "list.collapseAll" => tree.collapse_all(),
            "list.toggleExpand" => {
                let Some(index) = focus else { return };
                if rows[index].is_dir || rows[index].has_nests {
                    tree.toggle(&paths[index]);
                } else {
                    return self.select_explorer_focus(true, window, cx);
                }
            }
            "list.select" => return self.select_explorer_focus(false, window, cx),
            "list.selectAndPreserveFocus" => return self.select_explorer_focus(true, window, cx),
            "list.focusParent" => {
                let Some(parent) = focus.and_then(|index| crate::list_tree::parent(&rows.iter().map(|row| row.depth).collect::<Vec<_>>(), index)) else { return };
                tree.focus(Some(paths[parent].clone()));
            }
            // abstractTree.ts `collapse(focus, true)`, then the focus becomes the selection and the anchor.
            "list.collapseAllToFocus" => {
                let Some(index) = focus else { return };
                tree.collapse_recursive(&paths[index]);
                tree.set_selection(&paths[index]);
            }
            "list.showHover" => return self.show_explorer_focus_hover(window, cx),
            "list.expandSelectionDown" | "list.expandSelectionUp" => {
                let next = match (focus, id == "list.expandSelectionDown") {
                    (None, down) => if down { 0 } else { last },
                    (Some(index), true) => (index + 1).min(last),
                    (Some(index), false) => index.saturating_sub(1),
                };
                // `expandMultiSelection`: moving back into the selection drops the row left, else the new row joins.
                if focus != Some(next) {
                    let mut selected = tree.selected.clone();
                    match selected.contains(&paths[next]) {
                        true => selected.retain(|path| Some(path) != focus.map(|index| &paths[index])),
                        false => selected.push(paths[next].clone()),
                    }
                    tree.focused = Some(paths[next].clone());
                    tree.selected = selected;
                }
                self.reveal_explorer_focus(false);
                cx.notify();
                return;
            }
            "list.selectAll" => tree.select_all(),
            "list.toggleSelection" => {
                let Some(index) = focus else { return };
                match tree.selected.iter().position(|path| *path == paths[index]) {
                    Some(position) => drop(tree.selected.remove(position)),
                    None => tree.selected.push(paths[index].clone()),
                }
            }
            "list.clear" => {
                if tree.selected.len() <= 1 {
                    tree.focused = None;
                }
                tree.selected.clear();
                tree.clear_anchor();
            }
            _ => unreachable!("not an Explorer list command: {id}"),
        }
        self.reveal_explorer_focus(false);
        cx.notify();
    }

    /// listCommands.ts `selectElement`: a folder toggles, and the focused row becomes the selection; a file opens
    /// as the selection's keyboard event says (listService.ts `onSelectionFromKeyboard`): pinned with the focus, or
    /// as a preview keeping it.
    fn select_explorer_focus(&mut self, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.folder.as_mut() else { return };
        let Some(focused) = tree.focused.clone() else { return };
        let row = tree.rows().into_iter().find(|row| row.path == focused);
        let collapsible = row.is_some_and(|row| row.is_dir || row.has_nests);
        if collapsible {
            tree.toggle(&focused);
        }
        tree.set_selection(&focused);
        cx.notify();
        if !focused.is_dir() {
            self.open_file(focused, !preserve_focus, window, cx);
        }
    }

    /// listCommands.ts `list.showHover`: the focused row's path hover, placed by the row as it is drawn.
    fn show_explorer_focus_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.folder.as_ref() else { return };
        let rows = tree.rows();
        let Some(index) = tree.focused.as_ref().and_then(|focused| rows.iter().position(|row| row.path == *focused)) else { return };
        let path = rows[index].path.clone();
        let list = self.explorer.scroll.bounds();
        let padding = if self.explorer.find.is_open() { px(30.) } else { px(0.) };
        // The rows are 4px in from each side of the list.
        let origin = gpui::point(list.left() + px(4.), list.top() + padding + ROW_HEIGHT * index as f32 + self.explorer.scroll.offset().y);
        let bounds = gpui::Bounds::new(origin, gpui::size(list.size.width - px(8.), ROW_HEIGHT));
        let content = super::hover::HoverContent::Text(crate::quick_input::path_label(&path, None));
        self.show_managed_hover(("explorer-hover", index), content, super::hover::HoverOptions::mouse(), bounds, window, cx);
    }

    /// listWidget.ts `focusNextPage` and `focusPreviousPage`: the last (first) row of the page, else one page on.
    fn focus_explorer_page(&mut self, down: bool, focus: Option<usize>, paths: &[PathBuf], cx: &mut Context<Self>) {
        let viewport = self.explorer.scroll.bounds().size.height;
        let max_scroll_top = (ROW_HEIGHT * paths.len() as f32 - viewport).max(px(0.));
        let edge_row = |scroll_top: Pixels| -> usize {
            let index = |y: Pixels| ((y / ROW_HEIGHT).floor() as usize).min(paths.len() - 1);
            match down {
                true => index(scroll_top + viewport).saturating_sub(1),
                false if scroll_top <= px(0.) => 0,
                false => (index(scroll_top - px(1.)) + 1).min(paths.len() - 1),
            }
        };
        let scroll_top = -self.explorer.scroll.offset().y;
        let edge = edge_row(scroll_top);
        let on_page = match down {
            true => focus.is_none_or(|focus| edge > focus),
            false => focus.is_none_or(|focus| focus >= edge),
        };
        let target = if focus != Some(edge) && on_page {
            edge
        } else {
            let step = if down && focus.is_some_and(|focus| edge > focus) { viewport - ROW_HEIGHT } else { viewport };
            let next_scroll_top = if down { scroll_top + step } else { scroll_top - step }.clamp(px(0.), max_scroll_top);
            if next_scroll_top == scroll_top {
                return;
            }
            self.explorer.scroll.set_offset(gpui::point(px(0.), -next_scroll_top));
            edge_row(next_scroll_top)
        };
        let tree = self.folder.as_mut().expect("the Explorer has rows");
        tree.focus(Some(paths[target].clone()));
        self.reveal_explorer_focus(false);
        cx.notify();
    }
}
