//! editorTypePicker.ts with breadcrumbsControl.ts `_updateEditorTypeControl`: the breadcrumbs' dropdown of the editors
//! that can open the active editor's resource, which reopens it with another one or sets the default; a diff editor's
//! also opens either side.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, ClickEvent, Context, Div, Pixels, Stateful, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::custom_editors::RANK_EXCLUSIVE;
use super::host_requests::uri_parts;
use super::hover::{HoverContent, HoverOptions};
use super::output_view::bounds_tracker;
use crate::menus::MenuAction;

#[cfg(test)]
mod tests;

const REOPEN_PREFIX: &str = "editorType.reopen.";
const SET_DEFAULT_PREFIX: &str = "editorType.setDefault.";
const OPEN_TO_SIDE_PREFIX: &str = "editorType.openToSide.";
const OPEN_ORIGINAL: &str = "editorType.openOriginal";
const OPEN_MODIFIED: &str = "editorType.openModified";

/// `IAvailableEditorTypes`: the editors (id, label, provider) when there is more than one to switch between.
struct EditorTypes {
    resource: Value,
    /// A diff editor's original and modified resources; `resource` is the modified one.
    diff: Option<(Value, Value)>,
    current: String,
    editors: Vec<(String, String, String)>,
}

impl EditorTypes {
    /// `editorTypeDisplayLabel`: a diff's text editor is the "Text Diff Editor".
    fn label(&self, id: &str, label: &str) -> String {
        match self.diff.is_some() && id == "default" {
            true => "Text Diff Editor".to_owned(),
            false => label.to_owned(),
        }
    }
}

impl Workbench {
    /// `getAvailableEditorTypes` of the active editor, with `breadcrumbs.showEditorType` and
    /// `workbench.editor.hiddenEditorTypes`.
    fn available_editor_types(&self) -> Option<EditorTypes> {
        if self.setting("breadcrumbs.showEditorType") == false || self.editor_type_picker_pending.is_some() {
            return None;
        }
        let hidden = self.setting("workbench.editor.hiddenEditorTypes");
        let hidden: Vec<&str> = hidden.as_array().into_iter().flatten().filter_map(|id| id.as_str()).collect();
        let (resource, diff, current) = match self.editors.active_diff() {
            Some(diff) => {
                let modified = crate::editor::uri_value(&diff.modified.scheme, &diff.modified.path);
                let original = crate::editor::uri_value(&diff.original.scheme, &diff.original.path);
                (modified.clone(), Some((original, modified)), "default".to_owned())
            }
            None => {
                let (resource, current) = self.active_editor_type()?;
                (resource, None, current)
            }
        };
        let (scheme, path) = uri_parts(&resource).ok()?;
        let editors = self.resource_editors(scheme, &path, diff.is_some());
        if editors.iter().any(|(_, _, _, rank, _)| *rank == RANK_EXCLUSIVE) {
            return None;
        }
        let editors: Vec<(String, String, String)> = editors
            .into_iter()
            .filter(|(id, _, _, _, universal_option)| *id == current || (!universal_option && !hidden.contains(&id.as_str())))
            .map(|(id, label, provider, _, _)| (id, label, provider))
            .collect();
        (editors.len() > 1).then_some(EditorTypes { resource, diff, current, editors })
    }

    /// The dropdown at the breadcrumbs' right: the current editor's label and a chevron.
    pub(super) fn editor_type_picker(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let types = self.available_editor_types()?;
        let label = types.editors.iter().find(|(id, _, _)| *id == types.current).map_or(types.current.clone(), |(id, label, _)| types.label(id, label));
        let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
        let foreground = self.color("breadcrumb.foreground");
        let hover = self.color("breadcrumb.focusForeground");
        Some(
            div()
                .id("breadcrumbs-editor-type")
                .flex_none()
                .ml_auto()
                .max_w(gpui::relative(0.4))
                .flex()
                .items_center()
                .pl(px(8.))
                .pr(px(4.))
                .cursor_pointer()
                .text_size(px(11.))
                .bg(self.color("breadcrumb.background"))
                .text_color(foreground)
                .hover(move |picker| picker.text_color(hover))
                .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.clone()))
                .child(self.icon("chevron-down", 15., 13., foreground).ml(px(2.)))
                .child(bounds_tracker(bounds.clone()))
                .child(self.hover_target("breadcrumbs-editor-type-hover", HoverContent::Text(format!("Editor: {label}")), HoverOptions::mouse()))
                // contextMenuHandler.ts anchors the menu to the element: below it, its right edge where the left overflows.
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.show_editor_type_menu(bounds.get(), window, cx);
                })),
        )
    }

    /// `createEditorTypeActions`: one action per editor (the current one checked) that reopens with it, with Alt opens
    /// it to the side, then the "Set Default" submenu (the effective default checked).
    fn show_editor_type_menu(&mut self, anchor: Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(types) = self.available_editor_types() else { return };
        let resource = types.resource.clone();
        let diff = types.diff.is_some();
        let Ok((scheme, path)) = uri_parts(&resource) else { return };
        let (default_id, pattern) = self.default_editor(scheme, &path, true, diff);
        let label_with_source = |label: &str, provider: &str| match provider {
            "Built-in" => label.to_owned(),
            provider => format!("{label} - {provider}"),
        };
        let mut reopen = Vec::new();
        let mut set_default = Vec::new();
        for (id, label, provider) in &types.editors {
            let title = label_with_source(&types.label(id, label), provider);
            let reopen_id = format!("{REOPEN_PREFIX}{id}");
            let editor_id = id.clone();
            self.commands.register(&reopen_id, move |workbench, _, window, cx| workbench.reopen_active_editor_with(Some(editor_id.clone()), window, cx));
            // The current editor of a singleton input (a notebook or custom editor) cannot open twice.
            let alt = (*id != types.current || types.current == "default").then(|| {
                let side_id = format!("{OPEN_TO_SIDE_PREFIX}{id}");
                let (editor_id, side_resource) = (id.clone(), resource.clone());
                self.commands.register(&side_id, move |workbench, _, window, cx| {
                    workbench.activate_column(&serde_json::json!(-2));
                    workbench.open_with_resolver(&side_resource, Some(&editor_id), serde_json::json!({ "pinned": true, "preserveFocus": false }), window, cx)
                });
                (side_id, format!("{title} (Open to the Side)"))
            });
            reopen.push(MenuAction::Command { id: reopen_id, title: title.clone(), enabled: true, checked: *id == types.current, alt });
            let default_command = format!("{SET_DEFAULT_PREFIX}{id}");
            let editor_id = id.clone();
            let default_resource = resource.clone();
            self.commands.register(&default_command, move |workbench, _, window, cx| {
                workbench.set_default_editor(&default_resource, &editor_id, diff);
                workbench.reopen_active_editor_with(Some(editor_id.clone()), window, cx)
            });
            set_default.push(MenuAction::Command { id: default_command, title, enabled: true, checked: *id == default_id, alt: None });
        }
        let mut actions = reopen;
        actions.push(MenuAction::Separator);
        let submenu_title = match diff {
            true => format!("Set Default (Diff Only) for '{pattern}'"),
            false => format!("Set Default for '{pattern}'"),
        };
        actions.push(MenuAction::Submenu { title: submenu_title, actions: set_default, checked: false });
        if let Some((original, modified)) = types.diff {
            actions.push(MenuAction::Separator);
            for (command, title, side) in [(OPEN_ORIGINAL, "Open Original", original), (OPEN_MODIFIED, "Open Modified", modified)] {
                self.commands.register(command, move |workbench, _, window, cx| workbench.open_with_resolver(&side, None, json!({}), window, cx));
                actions.push(MenuAction::Command { id: command.to_owned(), title: title.to_owned(), enabled: true, checked: false, alt: None });
            }
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }
}
