//! editorTypePicker.ts with breadcrumbsControl.ts `_updateEditorTypeControl`: the breadcrumbs' dropdown of the editors
//! that can open the active editor's resource, which reopens it with another one or sets the default.

use gpui::{Bounds, ClickEvent, Context, Div, Stateful, Window, div, prelude::*, px};

use super::Workbench;
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
        let hidden = self.setting("workbench.editor.hiddenEditorTypes");
        let hidden: Vec<&str> = hidden.as_array().into_iter().flatten().filter_map(|id| id.as_str()).collect();
        let (resource, current) = self.active_editor_type()?;
        let (scheme, path) = uri_parts(&resource).ok()?;
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
                .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
                .child(self.icon("chevron-down", 15., 13., foreground).ml(px(2.)))
                .on_click(cx.listener(|workbench, event: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    let anchor = Bounds { origin: event.position(), size: gpui::Size::default() };
                    workbench.show_editor_type_menu(anchor, window, cx);
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
