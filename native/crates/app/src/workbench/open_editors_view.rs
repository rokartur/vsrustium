//! openEditorsView.ts: the Explorer's Open Editors view, the editors of every group in a 22px row each.

use gpui::{AnyElement, AppContext, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px};

use serde_json::{Value, json};

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
        // `.context-menu-visible .monaco-list.last-focused`: the list's context menu keeps the focused row outlined.
        let outlined = focused || self.context_menu.as_ref().is_some_and(|menu| menu.focus_to_return.as_ref() == Some(&self.open_editors_focus));
        let focused_row = self.open_editors_focused_row.or_else(|| self.open_editors_selected_row(&rows));
        let mut body = div()
            .id("open-editors")
            .track_focus(&self.open_editors_focus)
            .track_scroll(&self.open_editors_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col();
        for (position, row) in rows.into_iter().enumerate() {
            let row_focused = focused_row == Some(position);
            body = body.child(match row {
                Row::Group(group, ordinal) => self.open_editors_group_row(position, group, ordinal, row_focused && outlined, cx),
                Row::Editor(group, index) => self.open_editor_row(position, group, index, focused, row_focused, row_focused && outlined, cx),
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

    /// openEditorsView.ts `onListContextMenu`: `MenuId.OpenEditorsContext` with the right-clicked row focused and its
    /// context keys (`handleContextKeys`); the commands get the editor's resource and `{ groupId, editorIndex }`, a
    /// group row `{}` and `{ groupId }`. The row's group is activated first, as the port's commands act on it.
    fn show_open_editors_context_menu(&mut self, position: usize, row: Row, pointer: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let group = match row {
            Row::Group(group, _) | Row::Editor(group, _) => group,
        };
        self.activate_group(group);
        self.editors_changed(cx);
        self.open_editors_focused_row = Some(position);
        window.focus(&self.open_editors_focus.clone(), cx);
        let mut context = self.key_context(window);
        for key in ["resource", "resourceScheme", "resourceFilename", "resourceExtname", "resourceLangId", "hasResource", "isFileSystemResource"] {
            context.remove(key);
        }
        context.insert("autoSaveAfterShortDelayContext".into(), json!(self.auto_save_configuration(None).short_delay));
        context.insert("listDoubleSelection".into(), json!(false));
        context.insert("groupFocusedInOpenEditors".into(), json!(matches!(row, Row::Group(..))));
        let args = match row {
            Row::Group(group, _) => vec![json!({}), json!({ "groupId": group })],
            Row::Editor(group, index) => {
                let editor = &self.editors.editors[index];
                let (scheme, path) = (editor.document().scheme.clone(), editor.document().path.clone());
                context.insert("dirtyEditorFocusedInOpenEditors".into(), json!(self.editor_shows_dirty(editor)));
                context.insert("readonlyEditorFocusedInOpenEditors".into(), json!(self.readonly(&scheme, &path).is_some()));
                context.insert("resource".into(), json!(crate::editor::uri_string(&scheme, &path)));
                context.insert("resourceScheme".into(), json!(scheme));
                context.insert("resourceFilename".into(), json!(path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()));
                context.insert("resourceExtname".into(), json!(path.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default()));
                context.insert("hasResource".into(), json!(true));
                context.insert("isFileSystemResource".into(), json!(scheme == "file"));
                vec![editor.document().uri(), json!({ "groupId": group, "editorIndex": index })]
            }
        };
        let anchor = gpui::Bounds { origin: pointer, size: gpui::Size::default() };
        self.show_context_menu(crate::menus::MenuId::OpenEditorsContext, &context, anchor, args, window, cx);
    }

    /// editor.ts `preventEditorClose` with `EditorCloseMethod.MOUSE`: a sticky editor stays on a middle click while
    /// `workbench.editor.preventPinnedEditorClose` is `mouse` or `keyboardAndMouse`.
    pub(super) fn prevents_mouse_close(&self, editor: &crate::editor::OpenEditor) -> bool {
        editor.sticky && matches!(self.setting("workbench.editor.preventPinnedEditorClose").as_str(), Some("mouse" | "keyboardAndMouse"))
    }

    /// The list commands (listCommands.ts) on the Open Editors list: focus moves and reveals, Enter opens the focused
    /// row with the focus (`list.select`, pinned), Space opens it keeping the focus (`list.toggleExpand`, a preview).
    /// Answers false for the commands the list does not take.
    pub(super) fn open_editors_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.open_editors_rows();
        if rows.is_empty() {
            return true;
        }
        let last = rows.len() - 1;
        let focus = self.open_editors_focused_row.or_else(|| self.open_editors_selected_row(&rows)).filter(|row| *row <= last);
        let page = (f32::from(self.open_editors_scroll.bounds().size.height) / ITEM_HEIGHT).floor().max(1.) as usize;
        let to = match id {
            "list.focusDown" | "list.focusAnyDown" => focus.map_or(0, |row| (row + 1).min(last)),
            "list.focusUp" | "list.focusAnyUp" => focus.map_or(last, |row| row.saturating_sub(1)),
            "list.focusFirst" | "list.focusAnyFirst" => 0,
            "list.focusLast" | "list.focusAnyLast" => last,
            "list.focusPageDown" => focus.map_or(0, |row| (row + page - 1).min(last)),
            "list.focusPageUp" => focus.map_or(0, |row| row.saturating_sub(page - 1)),
            "list.select" | "list.toggleExpand" | "list.selectAndPreserveFocus" => {
                let Some(row) = focus else { return true };
                self.open_open_editors_row(rows[row], id != "list.select", window, cx);
                return true;
            }
            _ => return false,
        };
        self.open_editors_focused_row = Some(to);
        self.open_editors_scroll.scroll_to_item(to);
        cx.notify();
        true
    }

    /// `onDidOpen` of a row: an editor opens in its group (pinned unless the focus stays in the list), a group row
    /// activates its group.
    fn open_open_editors_row(&mut self, row: Row, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            Row::Editor(group, index) => {
                self.focus_group(group, window, cx);
                self.editors.activate(index);
                if !preserve_focus {
                    self.editors.pin(index);
                }
                self.editors_changed(cx);
            }
            Row::Group(group, _) => self.focus_group(group, window, cx),
        }
        if preserve_focus {
            window.focus(&self.open_editors_focus.clone(), cx);
        }
    }

    /// The list's context keys while it has focus (`handleContextKeys`, WorkbenchList's `listFocus`).
    pub(super) fn open_editors_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        if !self.open_editors_focus.is_focused(window) {
            return;
        }
        for key in ["openEditorsFocus", "explorerViewletFocus", "sideBarFocus", "listFocus"] {
            context.insert(key.into(), json!(true));
        }
    }

    /// The row of the active group's active editor: the list's selection (`focusActiveEditor`).
    fn open_editors_selected_row(&self, rows: &[Row]) -> Option<usize> {
        let active = Row::Editor(self.editor_group_id(), self.editors.active_index()?);
        rows.iter().position(|row| *row == active)
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
    fn open_editors_group_row(&self, position: usize, group: u32, ordinal: usize, focused: bool, cx: &mut Context<Self>) -> AnyElement {
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
            .children(self.open_editors_focus_outline(focused))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_size(px(11.)).font_weight(gpui::FontWeight::BOLD).child(format!("GROUP {ordinal}")))
            .child(actions)
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_open_editors_context_menu(position, Row::Group(group, ordinal), event.position, window, cx)))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedTab, window, cx| workbench.move_dragged_tab(dragged, group, None, window, cx)))
            .into_any_element()
    }

    /// `OpenEditorRenderer`: the close action (shown on hover or focus, a filled circle while the editor is dirty), or
    /// on a sticky editor the always shown unpin action, then icon, name (italic while in preview) and folder. A click
    /// opens the editor in its group, a middle click closes it, a drop moves the dragged editor before it.
    fn open_editor_row(&self, position: usize, group: u32, index: usize, list_focused: bool, focused: bool, outlined: bool, cx: &mut Context<Self>) -> AnyElement {
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
        // iconlabel.css `.label-description`: 0.9em at 0.7 opacity (0.95 in light themes), 1 in a focused or selected row, 0.5em after the name.
        let opacity = match (selected || focused, self.theme.kind) {
            (true, _) => 1.,
            (false, ThemeKind::Light) => 0.95,
            (false, _) => 0.7,
        };
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
        // `OpenEditorRenderer` passes `explorer.decorations` as the label's `fileDecorations`; the focused list's
        // selection foreground wins over the decoration's color.
        let decoration = (editor.document().scheme == "file").then(|| self.file_decoration(&editor.document().path, false)).flatten();
        let decoration_color = self.decoration_color(decoration.as_ref(), self.setting("explorer.decorations.colors") != json!(false)).filter(|_| !(selected && list_focused));
        let badge_color = decoration_color.unwrap_or(foreground);
        let badge = self.decoration_badge(decoration.as_ref(), badge_color).filter(|_| self.setting("explorer.decorations.badges") != json!(false));
        let badge = badge.map(|badge| if selected && list_focused { badge.text_color(foreground) } else { badge });
        let label = label.when_some(decoration_color, |label, color| label.text_color(color));
        // openeditors.css: a dirty row keeps its action bar, the close icon drawn as `circle-filled` until hovered; a
        // sticky row's unpin action (`pinned`, `pinned-dirty` while dirty) always shows.
        let sticky = editor.sticky;
        let (action_icon, dirty_icon) = if sticky { ("pinned", "pinned-dirty") } else { ("close", "circle-filled") };
        let action = self
            .icon(action_icon, 20., 16., foreground)
            .id(("open-editors-close", position))
            .cursor_pointer()
            .when(dirty, |action| action.relative().invisible().group_hover(name.clone(), |action| action.visible()))
            .on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.activate_group(group);
                match sticky {
                    true => drop(workbench.execute_command("workbench.action.unpinEditor", vec![json!({ "groupId": group, "editorIndex": index })], window, cx)),
                    false => drop(workbench.close_editors(vec![index], window, cx)),
                }
            }));
        let actions = self
            .open_editors_action_bar(&name, focused || dirty || sticky)
            .relative()
            .child(action)
            .when(dirty, |actions| actions.child(self.icon(dirty_icon, 20., 16., foreground).absolute().left_0().group_hover(name.clone(), |icon| icon.invisible())));
        let row = self
            .open_editors_row(position, &name)
            .when(selected, |row| row.bg(selection))
            .text_color(foreground)
            .children(self.open_editors_focus_outline(outlined))
            .child(actions)
            .child(label)
            .children(badge)
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
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_open_editors_context_menu(position, Row::Editor(group, index), event.position, window, cx)))
            // `onMouseMiddleClick`: closes the editor unless `workbench.editor.preventPinnedEditorClose` keeps it.
            .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, _: &gpui::MouseUpEvent, window, cx| {
                workbench.activate_group(group);
                if !workbench.prevents_mouse_close(&workbench.editors.editors[index]) {
                    drop(workbench.close_editors(vec![index], window, cx));
                }
            }))
            .on_drag(self.dragged_tab(group, index), |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_drop(cx.listener(move |workbench, dragged: &DraggedTab, window, cx| workbench.move_dragged_tab(dragged, group, Some(index), window, cx)));
        row.into_any_element()
    }

    /// `.monaco-list:focus .monaco-list-row.focused`: the focused row's outline while the list has focus.
    fn open_editors_focus_outline(&self, shown: bool) -> Option<Div> {
        let outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| shown)?;
        Some(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(outline))
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
            // The modern list's rows sit 4px in from the view's edges, rounded.
            .mx(px(4.))
            .rounded(px(4.))
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
