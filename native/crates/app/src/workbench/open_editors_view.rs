//! openEditorsView.ts: the Explorer's Open Editors view, the editors of every group in a 22px row each.

use gpui::{AnyElement, AppContext, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px};

use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use super::editor_groups::DraggedTab;
use super::hover::{HoverContent, HoverOptions};
use super::view_panes::{OPEN_EDITORS_VIEW, PaneContainer, ViewPane};
use crate::theme::ThemeKind;

/// `OpenEditorsDelegate.ITEM_HEIGHT`.
const ITEM_HEIGHT: f32 = 22.;
/// `OpenEditorsView.DEFAULT_VISIBLE_OPEN_EDITORS`, the default of `explorer.openEditors.visible`.
const DEFAULT_VISIBLE_OPEN_EDITORS: u64 = 9;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Row {
    /// A group and its 1-based place in grid order (`IEditorGroup.label`).
    Group(u32, usize),
    /// An editor: its group and its index there.
    Editor(u32, usize),
}

impl Workbench {
    /// explorerViewlet.ts `focusCommand`: `openView` shows the view, expands it and focuses it.
    pub(super) fn register_open_editors_commands(&mut self) {
        self.commands.register("workbench.files.action.focusOpenEditorsView", |workbench, _, window, cx| {
            drop(workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx));
            if workbench.view_hidden("explorer", OPEN_EDITORS_VIEW) {
                workbench.toggle_view_visibility("explorer", OPEN_EDITORS_VIEW, cx);
                let height = workbench.explorer_panes.view.as_ref().map(|view| view.height());
                if let Some(height) = height {
                    workbench.layout_container_panes(height, cx);
                }
            }
            workbench.expand_pane(PaneContainer::Explorer, OPEN_EDITORS_VIEW, cx);
            window.focus(&workbench.open_editors_focus.clone(), cx);
            Outcome::done()
        });
        // `SAVE_ALL_IN_GROUP_COMMAND_ID`: the active group's dirty editors, untitled ones included.
        self.commands.register("workbench.files.action.saveAllInGroup", |workbench, _, _, cx| {
            let saves = workbench.dirty_editors().into_iter().map(|index| workbench.save_editor(index, cx)).collect();
            super::actions::when_saved(saves, Value::Bool, cx)
        });
    }

    /// `getElements`: the groups in grid order, each one's row before its editors while there are two or more.
    pub(super) fn open_editors_rows(&self) -> Vec<Row> {
        let groups = self.groups_in_grid_order();
        let sort_order = self.setting("explorer.openEditors.sortOrder");
        let mut rows = Vec::new();
        for (ordinal, &group) in groups.iter().enumerate() {
            if groups.len() > 1 {
                rows.push(Row::Group(group, ordinal + 1));
            }
            let editors = self.editors.group_tabs(group).0;
            let mut indices: Vec<usize> = (0..editors.len()).collect();
            let name = |index: usize| self.editor_name(&editors[index]);
            match sort_order.as_str() {
                Some("alphabetical") => indices.sort_by(|&a, &b| crate::explorer::compare_file_names(&name(a), &name(b))),
                // Non-file editors first, by their resource, then the files by path.
                Some("fullPath") => indices.sort_by_key(|&index| {
                    let document = editors[index].document();
                    (document.scheme == "file", document.scheme.clone(), document.path.to_string_lossy().to_lowercase())
                }),
                _ => {}
            }
            rows.extend(indices.into_iter().map(|index| Row::Editor(group, index)));
        }
        rows
    }

    /// `updateSize`: the body is at least `explorer.openEditors.visible` rows (fewer while there are fewer) and, beside
    /// other views, at most as many rows as there are (or `explorer.openEditors.minVisible`).
    pub(super) fn limit_open_editors_pane(&mut self) {
        let count = self.open_editors_rows().len() as f32;
        let visible = self.setting("explorer.openEditors.visible").as_u64().unwrap_or(DEFAULT_VISIBLE_OPEN_EDITORS).max(1) as f32;
        let min_visible = self.setting("explorer.openEditors.minVisible").as_u64().unwrap_or(0) as f32;
        let panes = self.panes_mut(PaneContainer::Explorer);
        let Some(view) = panes.view.as_mut() else { return };
        let Some(index) = view.ids().iter().position(|&id| id == OPEN_EDITORS_VIEW) else { return };
        let maximum = if view.ids().len() <= 1 { f32::INFINITY } else { count.max(min_visible) * ITEM_HEIGHT };
        view.set_body_limits(index, visible.min(count) * ITEM_HEIGHT, maximum);
    }

    pub(super) fn open_editors_pane(&self, window: &Window, cx: &mut Context<Self>) -> ViewPane {
        let rows = self.open_editors_rows();
        let focused = self.open_editors_focus.is_focused(window);
        let mut body = div().id("open-editors").track_focus(&self.open_editors_focus).overflow_y_scroll().flex_1().min_h_0().flex().flex_col();
        for (position, row) in rows.into_iter().enumerate() {
            body = body.child(match row {
                Row::Group(group, ordinal) => self.open_editors_group_row(position, group, ordinal, cx),
                Row::Editor(group, index) => self.open_editor_row(position, group, index, focused, cx),
            });
        }
        ViewPane {
            id: OPEN_EDITORS_VIEW,
            title: "Open Editors".into(),
            description: None,
            actions: Some(self.open_editors_title_actions(window, cx).into_any_element()),
            show_actions: focused,
            body: body.into_any_element(),
        }
    }

    /// `renderHeaderTitle`'s dirty count badge, hidden at 0; a click opens the first dirty editor in grid order.
    pub(super) fn open_editors_dirty_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.dirty_working_copy_count();
        if count == 0 {
            return None;
        }
        let badge = div()
            .id("open-editors-dirty-count")
            .relative()
            .flex_none()
            .ml(px(6.))
            .min_w(px(18.))
            .px(px(4.))
            .py(px(2.))
            .rounded(px(2.))
            .when_some(self.theme.color("contrastBorder"), |badge, border| badge.border_1().border_color(border))
            .text_size(px(11.))
            .line_height(px(13.))
            .font_weight(gpui::FontWeight::NORMAL)
            .cursor_pointer()
            .bg(self.color("badge.background"))
            .text_color(self.color("badge.foreground"))
            .child(format!("{count} unsaved"))
            .child(self.hover_target("open-editors-dirty-count-hover", HoverContent::Text("Open Unsaved Editor".into()), HoverOptions::element()))
            .on_click(cx.listener(|workbench, _: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.open_first_dirty_editor(window, cx);
            }));
        Some(badge.into_any_element())
    }

    /// `openFirstDirtyEditor`: the first dirty editor in grid order, opened pinned.
    fn open_first_dirty_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for group in self.groups_in_grid_order() {
            let Some(index) = self.editors.group_tabs(group).0.iter().position(|editor| self.editor_shows_dirty(editor)) else { continue };
            self.focus_group(group, window, cx);
            self.editors.activate(index);
            self.editors.pin(index);
            self.editors_changed(cx);
            return;
        }
    }

    /// The `view/title` navigation actions of openEditorsView.ts, by order: New Untitled Text File, Toggle Vertical/
    /// Horizontal Editor Layout (with two or more groups), Save All, Close All.
    fn open_editors_title_actions(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        let multiple_groups = self.groups_in_grid_order().len() > 1;
        let actions = [
            ("new-file", "New Untitled Text File", "workbench.action.files.newUntitledFile", true),
            ("editor-layout", "Toggle Vertical/Horizontal Editor Layout", "workbench.action.toggleEditorGroupLayout", multiple_groups),
            ("save-all", "Save All", "workbench.action.files.saveAll", true),
            ("close-all", "Close All Editors", "workbench.action.closeAllEditors", true),
        ];
        let hover = self.color("toolbar.hoverBackground");
        let foreground = self.foreground_or("icon.foreground");
        let mut bar = div().flex().gap(px(4.));
        for (icon, title, command, shown) in actions {
            if !shown {
                continue;
            }
            bar = bar.child(
                self.icon(icon, 20., 16., foreground)
                    .id(command)
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|action| action.bg(hover))
                    .on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        workbench.execute_command(command, Vec::new(), window, cx);
                    }))
                    .child(self.action_hover(title, command, HoverOptions::action_item())),
            );
        }
        bar
    }

    /// `EditorGroupRenderer`: the group's label in bold capitals, then Save All in Group and Close Group
    /// (shown on hover); a drop appends to the group.
    fn open_editors_group_row(&self, position: usize, group: u32, ordinal: usize, cx: &mut Context<Self>) -> AnyElement {
        let name = SharedString::from(format!("open-editors-row-{position}"));
        let mut actions = self.open_editors_action_bar(&name, false);
        for (icon, command) in [("save-all", "workbench.files.action.saveAllInGroup"), ("close-all", "workbench.action.closeGroup")] {
            actions = actions.child(
                self.icon(icon, 23., 16., self.foreground_or("icon.foreground"))
                    .id((command, position))
                    .h(px(ITEM_HEIGHT))
                    .cursor_pointer()
                    .on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        workbench.activate_group(group);
                        drop(workbench.execute_command(command, Vec::new(), window, cx));
                    })),
            );
        }
        self.open_editors_row(position, &name)
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_size(px(11.)).font_weight(gpui::FontWeight::BOLD).child(format!("GROUP {ordinal}")))
            .child(actions)
            .on_drop(cx.listener(move |workbench, dragged: &DraggedTab, window, cx| workbench.move_dragged_tab(dragged, group, None, window, cx)))
            .into_any_element()
    }

    /// `OpenEditorRenderer`: the close action (shown on hover or focus, a filled circle while the editor is dirty), then
    /// icon, name (italic while in preview) and folder. A click opens the editor in its group, a drop moves the dragged
    /// editor before it.
    fn open_editor_row(&self, position: usize, group: u32, index: usize, list_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.editors.group_tabs(group).0[index];
        let name = SharedString::from(format!("open-editors-row-{position}"));
        // The list's selection and focus follow the active editor of the active group.
        let selected = group == self.editor_group_id() && self.editors.group_tabs(group).1 == Some(index);
        let (selection, selection_foreground) = match list_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let foreground = match selected {
            true => selection_foreground.unwrap_or(self.foreground_or("sideBar.foreground")),
            false => self.foreground_or("sideBar.foreground"),
        };
        let dirty = self.editor_shows_dirty(editor);
        let description = match editor.document().scheme.as_str() {
            "file" => editor.document().path.parent().map(|folder| crate::quick_input::path_label(folder, self.workspace())).filter(|label| !label.is_empty()),
            _ => self.untitled_description(editor),
        };
        // iconlabel.css `.label-description`: 0.9em at 0.7 opacity (0.95 in light themes), 0.5em after the name.
        let opacity = if self.theme.kind == ThemeKind::Light { 0.95 } else { 0.7 };
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .when(editor.preview, |label| label.italic())
            .children(self.document_icon(editor, ITEM_HEIGHT, foreground).map(|icon| icon.mr(px(6.))))
            .child(self.editor_name(editor))
            .children(description.map(|description| div().ml(px(6.5)).text_size(px(11.7)).opacity(opacity).child(description)));
        // openeditors.css: a dirty row keeps its action bar, the close icon drawn as `circle-filled` until hovered.
        let close = self
            .icon("close", 20., 16., foreground)
            .id(("open-editors-close", position))
            .cursor_pointer()
            .when(dirty, |close| close.relative().invisible().group_hover(name.clone(), |close| close.visible()))
            .on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.activate_group(group);
                drop(workbench.close_editors(vec![index], window, cx));
            }));
        let actions = self
            .open_editors_action_bar(&name, (selected && list_focused) || dirty)
            .relative()
            .child(close)
            .when(dirty, |actions| actions.child(self.icon("circle-filled", 20., 16., foreground).absolute().left_0().group_hover(name.clone(), |icon| icon.invisible())));
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| selected && list_focused);
        let row = self
            .open_editors_row(position, &name)
            .when(selected, |row| row.bg(selection))
            .text_color(foreground)
            .children(focus_outline.map(|outline| div().absolute().inset_0().border_1().border_color(outline)))
            .child(actions)
            .child(label)
            .child(self.editor_title_hover(("open-editors-hover", position), editor))
            .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                workbench.focus_group(group, window, cx);
                workbench.editors.activate(index);
                if event.click_count() == 2 {
                    workbench.editors.pin(index);
                }
                workbench.editors_changed(cx);
                window.focus(&workbench.open_editors_focus.clone(), cx);
            }))
            // shortcut: the row shows the tab's EditorTitleContext menu in its group, not MenuId.OpenEditorsContext; port that menu when its compare and save items are needed here.
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                workbench.activate_group(group);
                workbench.editors_changed(cx);
                workbench.show_tab_context_menu(index, event.position, window, cx);
            }))
            .on_drag(self.dragged_tab(group, index), |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedTab, window, cx| workbench.move_dragged_tab(dragged, group, Some(index), window, cx)));
        row.into_any_element()
    }

    /// `.monaco-action-bar` at the row's start: hidden until the row is hovered, unless `shown` (focused or dirty).
    fn open_editors_action_bar(&self, name: &SharedString, shown: bool) -> Div {
        div().flex_none().flex().items_center().when(!shown, |actions| actions.invisible().group_hover(name.clone(), |actions| actions.visible()))
    }

    /// `.open-editors .monaco-list-row`: 22px, 8px before the content, the list's hover and drop colors.
    fn open_editors_row(&self, position: usize, name: &SharedString) -> gpui::Stateful<Div> {
        let drop = self.color("list.dropBackground");
        div()
            .id(("open-editors-row", position))
            .group(name.clone())
            .relative()
            .flex_none()
            .h(px(ITEM_HEIGHT))
            .line_height(px(ITEM_HEIGHT))
            .pl(px(8.))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop))
    }
}

#[cfg(test)]
mod tests;
