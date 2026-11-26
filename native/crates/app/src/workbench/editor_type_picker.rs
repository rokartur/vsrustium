//! The breadcrumbs' editor type control (breadcrumbsControl.ts `_updateEditorTypeControl`, editorTypePicker.ts): the
//! active editor's type with a chevron at the end of the breadcrumbs while its resource has several editors, and the
//! menu that reopens it with another one, opens one to the side or sets the default.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gpui::{AnyElement, Bounds, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, canvas, div, px};
use serde_json::{Value, json};

use super::Workbench;
use super::custom_editors::{EditorMatch, TEXT_EDITOR_ID};
use super::hover::{HoverContent, HoverOptions};
use super::webview_panels::WEBVIEW_PANEL_SCHEME;
use crate::commands::Outcome;
use crate::menus::MenuAction;

/// `SplitSettingsEditorInput.EDITOR_ID`, `SideBySideEditorInput.ID` kept for existing associations.
pub(super) const SPLIT_SETTINGS_EDITOR_ID: &str = "workbench.editorinputs.sidebysideEditorInput";
/// editorResolverService.ts `hiddenEditorTypesSettingId`.
const HIDDEN_EDITOR_TYPES: &str = "workbench.editor.hiddenEditorTypes";

#[derive(Default)]
pub(super) struct EditorTypePicker {
    /// The control's bounds as last painted: the menu's anchor (`getAnchor: () => editorTypeNode`).
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The editors the open menu offers, which its commands act on.
    shown: Option<AvailableEditorTypes>,
    /// How many `editorType.*.<index>` commands are registered.
    commands: usize,
}

/// editorTypePicker.ts `IAvailableEditorTypes`.
#[derive(Clone)]
pub(super) struct AvailableEditorTypes {
    pub(super) resource: Value,
    pub(super) current: String,
    pub(super) editors: Vec<EditorMatch>,
    /// `editorMatches.defaultRule`: its editor id and its association pattern.
    pub(super) default_id: String,
    pub(super) glob: String,
}

/// `getAvailableEditorTypes` over the resource's matches: the unconfigured editors registered for `*` with `option`
/// priority and the hidden ones are left out unless active; nothing to switch between gives `None`.
pub(super) fn available_editors(matches: Vec<EditorMatch>, current: &str, hidden: &[String]) -> Option<Vec<EditorMatch>> {
    let editors: Vec<EditorMatch> = matches.into_iter().filter(|editor| editor.id == current || !(editor.is_universal_option() || hidden.contains(&editor.id))).collect();
    (editors.len() > 1).then_some(editors)
}

/// `labelWithSource`: an extension's editor names its extension, a built-in one does not.
fn label_with_source(editor: &EditorMatch) -> String {
    match editor.detail.as_deref() {
        Some(detail) if detail != "Built-in" => format!("{} - {detail}", editor.label),
        _ => editor.label.clone(),
    }
}

impl Workbench {
    /// The active editor's resource, editor id and its available editor types.
    pub(super) fn available_editor_types(&self) -> Option<AvailableEditorTypes> {
        let tab = self.editors.active_tab()?;
        // shortcut: diff editors offer no types; the port's resolver has no `createDiffEditorInput` per editor yet.
        if self.editors.active_diff().is_some() || self.editors.active_merge().is_some() {
            return None;
        }
        let panel = self.webview_panels.panels.iter().find(|panel| tab.document().is(&panel.scheme, &panel.path));
        let (resource, current) = match panel {
            Some(panel) => (panel.custom.as_ref()?.resource.clone(), panel.view_type.clone()),
            None if self.editors.side_by_sides.contains_key(&tab.model_id) => (tab.document().uri(), SPLIT_SETTINGS_EDITOR_ID.to_owned()),
            None if tab.document().scheme == WEBVIEW_PANEL_SCHEME => return None,
            None => (tab.document().uri(), TEXT_EDITOR_ID.to_owned()),
        };
        let (scheme, path) = super::host_requests::uri_parts(&resource).ok()?;
        let mut matches = self.editor_matches(scheme, &path);
        let split = self.split_settings_match(scheme, &path);
        let (mut default_id, mut glob) = self.default_editor(scheme, &path, true);
        if let Some(split) = split {
            // The split settings editor ranks as the text editor and its glob is the longer one.
            if (default_id.clone(), glob.clone()) == self.default_editor(scheme, &path, false) && default_id == TEXT_EDITOR_ID {
                (default_id, glob) = (split.id.clone(), split.glob.clone());
            }
            matches.insert(0, split);
        }
        let hidden: Vec<String> = serde_json::from_value(self.setting(HIDDEN_EDITOR_TYPES)).unwrap_or_default();
        let editors = available_editors(matches, &current, &hidden)?;
        Some(AvailableEditorTypes { resource, current, editors, default_id, glob })
    }

    /// preferencesContribution.ts: `**/settings.json` opens in the split settings editor while
    /// `workbench.settings.useSplitJSON` or `openDefaultSettings` is on.
    fn split_settings_match(&self, scheme: &str, path: &Path) -> Option<EditorMatch> {
        let enabled = self.setting("workbench.settings.useSplitJSON") == json!(true) || self.setting("workbench.settings.openDefaultSettings") == json!(true);
        let matches = super::custom_editors::resource_matcher(scheme, path)?;
        (enabled && matches("**/settings.json")).then(|| EditorMatch { id: SPLIT_SETTINGS_EDITOR_ID.into(), label: "Split Settings Editor".into(), detail: None, rank: super::custom_editors::RANK_BUILTIN, glob: "**/settings.json".into() })
    }

    /// `.breadcrumbs-editor-type` (breadcrumbscontrol.css): the current type's label and a chevron, 11px, at the
    /// breadcrumbs' end; a click shows the editor type menu.
    pub(super) fn editor_type_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.setting("breadcrumbs.showEditorType") == json!(false) {
            return None;
        }
        let available = self.available_editor_types()?;
        let label = available.editors.iter().find(|editor| editor.id == available.current).map_or(available.current.clone(), |editor| editor.label.clone());
        let foreground = self.color("breadcrumb.foreground");
        let focus_foreground = self.color("breadcrumb.focusForeground");
        let bounds = self.editor_type_picker.bounds.clone();
        let measure = canvas(move |element, _, _| bounds.set(element), |_, _, _, _| ()).absolute().inset_0();
        let chevron = div()
            .ml(px(2.))
            .relative()
            .child(self.icon("chevron-down", 13., 13., foreground).group_hover("breadcrumbs-editor-type", |icon| icon.invisible()))
            .child(div().absolute().inset_0().invisible().group_hover("breadcrumbs-editor-type", |icon| icon.visible()).child(self.icon("chevron-down", 13., 13., focus_foreground)));
        let control = div()
            .id("breadcrumbs-editor-type")
            .group("breadcrumbs-editor-type")
            .relative()
            .flex_none()
            .max_w(gpui::relative(0.4))
            .h_full()
            .flex()
            .items_center()
            .pl(px(8.))
            .pr(px(4.))
            .cursor_pointer()
            .text_size(px(11.))
            .bg(self.color("breadcrumb.background"))
            .text_color(foreground)
            .hover(|control| control.text_color(focus_foreground))
            .child(measure)
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.clone()))
            .child(chevron)
            .child(self.hover_target("breadcrumbs-editor-type-hover", HoverContent::Text(format!("Editor: {label}")), HoverOptions::mouse()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_editor_type_menu(window, cx);
            }));
        Some(control.into_any_element())
    }

    /// `createEditorTypeActions`: an item per editor that reopens the active editor with it (the current one checked,
    /// with Alt "(Open to the Side)" unless it is a custom editor), then "Set Default for '<glob>'".
    fn show_editor_type_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(available) = self.available_editor_types() else { return };
        while self.editor_type_picker.commands < available.editors.len() {
            let index = self.editor_type_picker.commands;
            self.commands.register(format!("editorType.reopen.{index}"), move |workbench, _, window, cx| workbench.run_editor_type(index, EditorTypeAction::Reopen, window, cx));
            self.commands.register(format!("editorType.openToSide.{index}"), move |workbench, _, window, cx| workbench.run_editor_type(index, EditorTypeAction::OpenToSide, window, cx));
            self.commands.register(format!("editorType.setDefault.{index}"), move |workbench, _, window, cx| workbench.run_editor_type(index, EditorTypeAction::SetDefault, window, cx));
            self.editor_type_picker.commands += 1;
        }
        let singleton = available.current != TEXT_EDITOR_ID && available.current != SPLIT_SETTINGS_EDITOR_ID;
        let mut actions = Vec::new();
        for (index, editor) in available.editors.iter().enumerate() {
            let label = label_with_source(editor);
            let current = editor.id == available.current;
            let alt = (!(current && singleton)).then(|| (format!("editorType.openToSide.{index}"), format!("{label} (Open to the Side)")));
            actions.push(MenuAction::Command { id: format!("editorType.reopen.{index}"), title: label, enabled: true, checked: current, alt });
        }
        let defaults = available.editors.iter().enumerate();
        let set_default = defaults.map(|(index, editor)| MenuAction::Command { id: format!("editorType.setDefault.{index}"), title: label_with_source(editor), enabled: true, checked: editor.id == available.default_id, alt: None }).collect();
        actions.push(MenuAction::Separator);
        actions.push(MenuAction::Submenu { title: format!("Set Default for '{}'", available.glob), actions: set_default, checked: false });
        self.editor_type_picker.shown = Some(available);
        let anchor = self.editor_type_picker.bounds.get();
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    fn run_editor_type(&mut self, index: usize, action: EditorTypeAction, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(available) = self.editor_type_picker.shown.clone() else { return Outcome::done() };
        let editor_id = available.editors[index].id.clone();
        match action {
            EditorTypeAction::Reopen => self.reopen_editor_type(&available, &editor_id, window, cx),
            // The source stays: a copy opens in the group to the side, then takes the type there.
            EditorTypeAction::OpenToSide => {
                drop(self.execute_command("workbench.action.splitEditor", Vec::new(), window, cx));
                let mut beside = available.clone();
                beside.current = TEXT_EDITOR_ID.to_owned();
                self.reopen_editor_type(&beside, &editor_id, window, cx)
            }
            EditorTypeAction::SetDefault => {
                self.set_default_editor(&available.resource, &editor_id);
                self.reopen_editor_type(&available, &editor_id, window, cx)
            }
        }
    }

    /// editorCommands.ts `reopenActiveEditorWith`: the split settings editor turns into its settings file's text
    /// editor and back; any other type reopens through the resolver.
    pub(super) fn reopen_editor_type(&mut self, available: &AvailableEditorTypes, editor_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if available.current == editor_id {
            return Outcome::done();
        }
        let mut current = available.current.clone();
        if current == SPLIT_SETTINGS_EDITOR_ID {
            let entry = self.editors.active_tab().expect("the side by side editor is active").model_id;
            self.editors.remove_side_by_side(entry);
            self.editors_changed(cx);
            current = TEXT_EDITOR_ID.to_owned();
        }
        if editor_id == SPLIT_SETTINGS_EDITOR_ID {
            let Ok((_, path)) = super::host_requests::uri_parts(&available.resource) else { return Outcome::done() };
            if let Some(secondary) = self.split_settings_secondary(&path) {
                self.show_side_by_side(Path::new(secondary), window, cx);
            }
            return Outcome::done();
        }
        self.reopen_with(&available.resource, &current, editor_id, false, window, cx)
    }
}

#[derive(Clone, Copy)]
enum EditorTypeAction {
    Reopen,
    OpenToSide,
    SetDefault,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(id: &str, rank: u8, glob: &str) -> EditorMatch {
        EditorMatch { id: id.into(), label: id.into(), detail: None, rank, glob: glob.into() }
    }

    fn ids(editors: Option<Vec<EditorMatch>>) -> Option<Vec<String>> {
        editors.map(|editors| editors.into_iter().map(|editor| editor.id).collect())
    }

    // editorTypePicker.test.ts 'hidden editor types are omitted unless currently active'.
    #[test]
    fn hidden_editor_types_are_omitted_unless_currently_active() {
        let matches = || vec![editor("default", 3, "*"), editor("test.markdownEditor", 1, "*.test"), editor("test.markdownPreview", 1, "*.test")];
        let hidden = ["test.markdownPreview".to_owned()];
        assert_eq!(ids(available_editors(matches(), "test.markdownEditor", &hidden)), Some(vec!["default".into(), "test.markdownEditor".into()]));
        assert_eq!(ids(available_editors(matches(), "test.markdownPreview", &hidden)), Some(vec!["default".into(), "test.markdownEditor".into(), "test.markdownPreview".into()]));
    }

    // editorTypePicker.test.ts 'unconfigured universal optional matches only make the picker visible while active'.
    #[test]
    fn unconfigured_universal_optional_matches_only_make_the_picker_visible_while_active() {
        let matches = || vec![editor("default", 3, "*"), editor("test.universalPreview", 1, "*")];
        assert_eq!(ids(available_editors(matches(), "default", &[])), None);
        assert_eq!(ids(available_editors(matches(), "test.universalPreview", &[])), Some(vec!["default".into(), "test.universalPreview".into()]));
    }

    #[test]
    fn a_resource_with_only_the_text_editor_has_no_picker() {
        assert_eq!(ids(available_editors(vec![editor("default", 3, "*")], "default", &[])), None);
    }

    #[test]
    fn an_extension_editor_names_its_extension() {
        let mut preview = editor("markdown.preview", 1, "*.md");
        preview.label = "Markdown Preview".into();
        preview.detail = Some("Markdown Language Features".into());
        assert_eq!(label_with_source(&preview), "Markdown Preview - Markdown Language Features");
        preview.detail = Some("Built-in".into());
        assert_eq!(label_with_source(&preview), "Markdown Preview");
    }
}
