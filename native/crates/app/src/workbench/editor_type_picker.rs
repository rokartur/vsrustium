//! editorTypePicker.ts: the breadcrumbs' dropdown of the editors that can open the active editor's resource
//! (`breadcrumbs.showEditorType`), and its menu: reopen with each, Set Default, and Open Original and Open Modified
//! of a diff.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Context, MouseButton, MouseDownEvent, Pixels, Stateful, Div, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::custom_editors::EditorMatches;
use super::host_requests::uri_parts;
use super::hover::{HoverContent, HoverOptions};
use super::output_view::bounds_tracker;
use crate::commands::Outcome;
use crate::menus::MenuAction;

#[cfg(test)]
mod tests;

const TEXT_EDITOR_ID: &str = "default";

#[derive(Default)]
pub struct EditorTypePicker {
    /// Where the dropdown last painted, to anchor its menu.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The open menu's actions, run by `editorType.action.<index>`.
    actions: Vec<PickerAction>,
    /// How many `editorType.action.<index>` commands are registered.
    commands: usize,
}

#[derive(Clone)]
enum PickerAction {
    Reopen(String),
    OpenToSide(String),
    SetDefault(String),
    Open(Value),
}

/// `IAvailableEditorTypes`.
pub(super) struct AvailableEditorTypes {
    resource: Value,
    /// The original and modified resources of a diff editor.
    diff: Option<(Value, Value)>,
    current: String,
    matches: EditorMatches,
    /// The matches' editors left after the filters: id, label and detail.
    editors: Vec<(String, String, String)>,
}

impl AvailableEditorTypes {
    /// `editorTypeDisplayLabel`: the text editor of a diff is the "Text Diff Editor".
    fn display_label(&self, id: &str, label: &str) -> String {
        match self.diff.is_some() && id == TEXT_EDITOR_ID {
            true => "Text Diff Editor".to_owned(),
            false => label.to_owned(),
        }
    }

    fn current_label(&self) -> String {
        match self.editors.iter().find(|(id, _, _)| *id == self.current) {
            Some((id, label, _)) => self.display_label(id, label),
            None => self.current.clone(),
        }
    }

    /// `labelWithSource`: the provider after the label, except for built-in ones.
    fn label_with_source(&self, id: &str, label: &str, detail: &str) -> String {
        let label = self.display_label(id, label);
        match !detail.is_empty() && detail != "Built-in" {
            true => format!("{label} - {detail}"),
            false => label,
        }
    }
}

impl Workbench {
    /// `getAvailableEditorTypes` of the active editor; None when there is nothing to switch between.
    pub(super) fn available_editor_types(&self) -> Option<AvailableEditorTypes> {
        let (resource, diff, current) = match self.editors.active_diff() {
            Some(diff) => {
                let modified = crate::editor::uri_value(&diff.modified.scheme, &diff.modified.path);
                let original = crate::editor::uri_value(&diff.original.scheme, &diff.original.path);
                (modified.clone(), Some((original, modified)), TEXT_EDITOR_ID.to_owned())
            }
            None => {
                let (resource, current) = self.active_resource_and_editor_id()?;
                (resource, None, current)
            }
        };
        let (scheme, path) = uri_parts(&resource).ok()?;
        let matches = self.editor_matches(scheme, &path, diff.is_some());
        let hidden = self.setting("workbench.editor.hiddenEditorTypes");
        let hidden: Vec<&str> = hidden.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let editors: Vec<(String, String, String)> = matches
            .editors
            .iter()
            .filter(|editor| !editor.universal_option || editor.id == current)
            .filter(|editor| editor.id == current || !hidden.contains(&editor.id.as_str()))
            .map(|editor| (editor.id.clone(), editor.label.clone(), editor.detail.clone()))
            .collect();
        if matches.has_exclusive_match() || editors.len() <= 1 {
            return None;
        }
        Some(AvailableEditorTypes { resource, diff, current, matches, editors })
    }

    /// breadcrumbsControl.ts `_createEditorTypeControl`: the current editor's label and a chevron at the end of the
    /// breadcrumbs (breadcrumbscontrol.css `.breadcrumbs-editor-type`).
    pub(super) fn editor_type_control(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.setting("breadcrumbs.showEditorType") == false {
            return None;
        }
        let available = self.available_editor_types()?;
        let label = available.current_label();
        let foreground = self.color("breadcrumb.foreground");
        let hover_foreground = self.color("breadcrumb.focusForeground");
        let bounds = self.editor_type_picker.bounds.clone();
        Some(
            div()
                .id("breadcrumbs-editor-type")
                .group("breadcrumbs-editor-type")
                .relative()
                .flex_none()
                .max_w(gpui::relative(0.4))
                .flex()
                .items_center()
                .pl(px(8.))
                .pr(px(4.))
                .cursor_pointer()
                .text_size(px(11.))
                .bg(self.color("breadcrumb.background"))
                .text_color(foreground)
                .hover(|control| control.text_color(hover_foreground))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.clone()))
                .child(self.icon("chevron-down", 13., 13., foreground).group_hover("breadcrumbs-editor-type", |icon| icon.text_color(hover_foreground)).ml(px(2.)))
                .child(bounds_tracker(bounds.clone()))
                .child(self.hover_target("breadcrumbs-editor-type-hover", HoverContent::Text(format!("Editor: {label}")), HoverOptions::mouse()))
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.show_editor_type_picker(bounds.get(), window, cx);
                })),
        )
    }

    /// `_showEditorTypePicker` with `createEditorTypeActions`.
    fn show_editor_type_picker(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(available) = self.available_editor_types() else { return };
        let mut actions = Vec::new();
        let mut menu = Vec::new();
        let push = |action: PickerAction, actions: &mut Vec<PickerAction>| {
            actions.push(action);
            format!("editorType.action.{}", actions.len() - 1)
        };
        let command = |action: PickerAction, title: String, checked: bool, actions: &mut Vec<PickerAction>| MenuAction::Command { id: push(action, actions), title, enabled: true, checked, alt: None };
        for (id, label, detail) in &available.editors {
            let title = available.label_with_source(id, label, detail);
            let alt = Some((push(PickerAction::OpenToSide(id.clone()), &mut actions), format!("{title} (Open to the Side)")));
            menu.push(MenuAction::Command { id: push(PickerAction::Reopen(id.clone()), &mut actions), title, enabled: true, checked: *id == available.current, alt });
        }
        menu.push(MenuAction::Separator);
        let mut set_default = Vec::new();
        for (id, label, detail) in &available.editors {
            let title = available.label_with_source(id, label, detail);
            set_default.push(command(PickerAction::SetDefault(id.clone()), title, *id == available.matches.default_id, &mut actions));
        }
        let pattern = &available.matches.pattern;
        let submenu_title = match available.diff.is_some() {
            true => format!("Set Default (Diff Only) for '{pattern}'"),
            false => format!("Set Default for '{pattern}'"),
        };
        menu.push(MenuAction::Submenu { title: submenu_title, actions: set_default, checked: false });
        if let Some((original, modified)) = &available.diff {
            menu.push(MenuAction::Separator);
            menu.push(command(PickerAction::Open(original.clone()), "Open Original".into(), false, &mut actions));
            menu.push(command(PickerAction::Open(modified.clone()), "Open Modified".into(), false, &mut actions));
        }
        while self.editor_type_picker.commands < actions.len() {
            let index = self.editor_type_picker.commands;
            self.commands.register(format!("editorType.action.{index}"), move |workbench, _, window, cx| workbench.run_editor_type_action(index, window, cx));
            self.editor_type_picker.commands += 1;
        }
        self.editor_type_picker.actions = actions;
        // contextMenuHandler.ts anchors an element below its bottom left corner.
        let anchor = Bounds { origin: gpui::point(anchor.origin.x, anchor.origin.y + anchor.size.height), size: Default::default() };
        let context = self.key_context(window);
        self.show_context_menu_actions(menu, &context, anchor, Vec::new(), window, cx);
    }

    fn run_editor_type_action(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(action) = self.editor_type_picker.actions.get(index).cloned() else { return Outcome::done() };
        let Some(available) = self.available_editor_types() else { return Outcome::done() };
        match action {
            PickerAction::Reopen(editor_id) => self.reopen_active_editor_with(Some(editor_id), window, cx),
            PickerAction::OpenToSide(editor_id) => {
                let direction = self.side_by_side_direction();
                let group = self.group_in_direction(direction).unwrap_or_else(|| self.add_group_beside(direction));
                self.focus_group(group, window, cx);
                self.open_with_resolver(&available.resource, Some(&editor_id), json!({ "pinned": true }), window, cx)
            }
            // "Setting a default also reopens the active editor with that type".
            PickerAction::SetDefault(editor_id) => {
                self.set_default_editor_for(&available.resource, &editor_id, available.diff.is_some());
                self.reopen_active_editor_with(Some(editor_id), window, cx)
            }
            PickerAction::Open(resource) => self.open_with_resolver(&resource, None, json!({}), window, cx),
        }
    }
}
