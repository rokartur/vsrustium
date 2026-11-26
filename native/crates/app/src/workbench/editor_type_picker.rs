//! editorTypePicker.ts with breadcrumbsControl.ts `_updateEditorTypeControl`: the breadcrumbs' dropdown of the editors
//! that can open the active editor's resource, which reopens it with another one or sets the default.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Context, Div, Stateful, Window, div, prelude::*, px};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::host_requests::uri_parts;
use crate::menus::MenuAction;

const REOPEN_PREFIX: &str = "editorType.reopen.";
const SET_DEFAULT_PREFIX: &str = "editorType.setDefault.";
const OPEN_TO_SIDE_PREFIX: &str = "editorType.openToSide.";

/// `IAvailableEditorTypes`: the editors (id, label, provider) when there is more than one to switch between.
struct EditorTypes {
    current: String,
    editors: Vec<(String, String, String)>,
}

impl Workbench {
    /// `getAvailableEditorTypes` of the active editor, with `breadcrumbs.showEditorType` and
    /// `workbench.editor.hiddenEditorTypes`.
    fn available_editor_types(&self) -> Option<EditorTypes> {
        if self.setting("breadcrumbs.showEditorType") == false || self.editor_type_picker_pending.is_some() {
            return None;
        }
        let (resource, current) = self.active_editor_type()?;
        self.editor_types(&resource, current)
    }

    fn editor_types(&self, resource: &serde_json::Value, current: String) -> Option<EditorTypes> {
        let hidden = self.setting("workbench.editor.hiddenEditorTypes");
        let hidden: Vec<&str> = hidden.as_array().into_iter().flatten().filter_map(|id| id.as_str()).collect();
        let (scheme, path) = uri_parts(resource).ok()?;
        let editors: Vec<(String, String, String)> = self
            .resource_editors(scheme, &path)
            .into_iter()
            .filter(|(id, _, _, universal_option)| *id == current || (!universal_option && !hidden.contains(&id.as_str())))
            .map(|(id, label, provider, _)| (id, label, provider))
            .collect();
        (editors.len() > 1).then_some(EditorTypes { current, editors })
    }

    /// The dropdown at the breadcrumbs' right: the current editor's label and a chevron.
    pub(super) fn editor_type_picker(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let types = self.available_editor_types()?;
        let label = types.editors.iter().find(|(id, _, _)| *id == types.current).map_or(types.current.clone(), |(_, label, _)| label.clone());
        let foreground = self.color("breadcrumb.foreground");
        let hover = self.color("breadcrumb.focusForeground");
        // The menu's anchor is the control (`getAnchor: () => editorTypeNode`).
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let anchor = bounds.clone();
        Some(
            div()
                .id("breadcrumbs-editor-type")
                .relative()
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
                .child(super::output_view::bounds_tracker(bounds))
                .child(self.hover_target("breadcrumbs-editor-type", HoverContent::Text(format!("Editor: {label}")), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    cx.stop_propagation();
                    workbench.show_editor_type_menu(anchor.get(), window, cx);
                })),
        )
    }

    /// `createEditorTypeActions`: one action per editor (the current one checked) that reopens with it, with Alt opens
    /// it to the side, then the "Set Default" submenu (the effective default checked).
    fn show_editor_type_menu(&mut self, anchor: Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(types) = self.available_editor_types() else { return };
        let Some((resource, _)) = self.active_editor_type() else { return };
        let Ok((scheme, path)) = uri_parts(&resource) else { return };
        let (default_id, pattern) = self.default_editor(scheme, &path, true);
        let label_with_source = |label: &str, provider: &str| match provider {
            "Built-in" => label.to_owned(),
            provider => format!("{label} - {provider}"),
        };
        let mut reopen = Vec::new();
        let mut set_default = Vec::new();
        for (id, label, provider) in &types.editors {
            let title = label_with_source(label, provider);
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
                workbench.set_default_editor(&default_resource, &editor_id);
                workbench.reopen_active_editor_with(Some(editor_id.clone()), window, cx)
            });
            set_default.push(MenuAction::Command { id: default_command, title, enabled: true, checked: *id == default_id, alt: None });
        }
        let mut actions = reopen;
        actions.push(MenuAction::Separator);
        actions.push(MenuAction::Submenu { title: format!("Set Default for '{pattern}'"), actions: set_default, checked: false });
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use exthost::scan::ScannedExtension;
    use serde_json::json;

    use super::*;

    #[gpui::test]
    fn lists_the_editor_types_of_a_resource_as_get_available_editor_types_does(cx: &mut gpui::TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        let description = |custom_editors: serde_json::Value| json!({ "isBuiltin": false, "displayName": "Hex", "contributes": { "customEditors": custom_editors } });
        let editors = json!([
            { "viewType": "hex", "displayName": "Hex Editor", "priority": "option", "selector": [{ "filenamePattern": "*" }] },
            { "viewType": "md", "displayName": "Preview", "priority": "option", "selector": [{ "filenamePattern": "*.md" }] },
        ]);
        workbench.update(cx, |workbench, _| workbench.extensions = vec![ScannedExtension { id: "pub.hex".into(), location: PathBuf::new(), description: description(editors) }]);
        let types = |path: &str, current: &str, cx: &mut gpui::VisualTestContext| {
            let resource = json!({ "scheme": "file", "path": path });
            workbench.read_with(cx, |workbench, _| workbench.editor_types(&resource, current.to_owned()).map(|types| types.editors.into_iter().map(|(id, _, provider)| format!("{id} {provider}")).collect::<Vec<_>>()))
        };
        // The image preview's glob is longer than the text editor's `*`; the unconfigured `*` option editor is left out.
        assert_eq!(types("/a.png", "imagePreview.previewEditor", cx), Some(vec!["imagePreview.previewEditor Built-in".into(), "default Built-in".into()]));
        assert_eq!(types("/a.md", "default", cx), Some(vec!["default Built-in".into(), "md Hex".into()]));
        assert_eq!(types("/a.txt", "default", cx), None);
        assert_eq!(types("/a.txt", "hex", cx), Some(vec!["default Built-in".into(), "hex Hex".into()]));
    }
}
