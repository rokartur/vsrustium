//! editorTypePicker.ts with breadcrumbsControl.ts `_updateEditorTypeControl`: the breadcrumbs' dropdown of the editors
//! that can open the active editor's resource, which reopens it with another one or sets the default; a diff editor's
//! also opens either side.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gpui::{Bounds, ClickEvent, Context, Div, MouseButton, Pixels, Stateful, Window, canvas, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::custom_editors::{RANK_EXCLUSIVE, TEXT_EDITOR_ID};
use super::host_requests::uri_parts;
use super::hover::{HoverContent, HoverOptions};
use crate::commands::Outcome;
use crate::menus::MenuAction;

#[cfg(test)]
mod tests;

const REOPEN_PREFIX: &str = "editorType.reopen.";
const SET_DEFAULT_PREFIX: &str = "editorType.setDefault.";
const OPEN_TO_SIDE_PREFIX: &str = "editorType.openToSide.";
const OPEN_ORIGINAL: &str = "editorType.openOriginal";
const OPEN_MODIFIED: &str = "editorType.openModified";
/// `SplitSettingsEditorInput.EDITOR_ID`, `SideBySideEditorInput.ID` kept for existing associations.
const SPLIT_SETTINGS_EDITOR_ID: &str = "workbench.editorinputs.sidebysideEditorInput";

/// Each group's control bounds as last painted: the menu's anchor (`getAnchor: () => editorTypeNode`).
#[derive(Default)]
pub(super) struct EditorTypePicker {
    bounds: Rc<RefCell<HashMap<u32, Bounds<Pixels>>>>,
}

/// `IAvailableEditorTypes`: the editors (id, label, provider) when there is more than one to switch between, and
/// `editorMatches.defaultRule`'s editor id and glob.
#[derive(Clone)]
pub(super) struct EditorTypes {
    pub(super) resource: Value,
    /// A diff editor's original and modified resources; `resource` is the modified one.
    pub(super) diff: Option<(Value, Value)>,
    pub(super) current: String,
    pub(super) editors: Vec<(String, String, String)>,
    pub(super) default_id: String,
    pub(super) pattern: String,
}

/// `getAvailableEditorTypes` over the resource's editors (id, label, provider, unconfigured `*` option): the `*`
/// option editors and the hidden ones are left out unless active; nothing to switch between gives `None`.
fn available_editors(editors: Vec<(String, String, String, u8, bool)>, current: &str, hidden: &[&str]) -> Option<Vec<(String, String, String)>> {
    if editors.iter().any(|(_, _, _, rank, _)| *rank == RANK_EXCLUSIVE) {
        return None;
    }
    let editors: Vec<(String, String, String)> = editors
        .into_iter()
        .filter(|(id, _, _, _, universal_option)| id == current || (!universal_option && !hidden.contains(&id.as_str())))
        .map(|(id, label, provider, _, _)| (id, label, provider))
        .collect();
    (editors.len() > 1).then_some(editors)
}

/// `labelWithSource`: an extension's editor names its extension, a built-in one does not.
fn label_with_source(label: &str, provider: &str) -> String {
    match provider {
        "Built-in" => label.to_owned(),
        provider => format!("{label} - {provider}"),
    }
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
    /// `workbench.editor.hiddenEditorTypes`; a side by side settings editor is the split settings editor.
    pub(super) fn available_editor_types(&self) -> Option<EditorTypes> {
        if self.setting("breadcrumbs.showEditorType") == false {
            return None;
        }
        let hidden = self.setting("workbench.editor.hiddenEditorTypes");
        let hidden: Vec<&str> = hidden.as_array().into_iter().flatten().filter_map(|id| id.as_str()).collect();
        let (resource, diff, mut current) = match self.editors.active_diff() {
            Some(diff) => {
                let modified = crate::editor::uri_value(&diff.modified.scheme, &diff.modified.path);
                let original = crate::editor::uri_value(&diff.original.scheme, &diff.original.path);
                (modified.clone(), Some((original, modified)), TEXT_EDITOR_ID.to_owned())
            }
            None => {
                let (resource, current) = self.active_editor_type()?;
                (resource, None, current)
            }
        };
        if self.editors.active_tab().is_some_and(|tab| self.editors.side_by_sides.contains_key(&tab.model_id)) {
            current = SPLIT_SETTINGS_EDITOR_ID.to_owned();
        }
        let (scheme, path) = uri_parts(&resource).ok()?;
        let is_diff = diff.is_some();
        let mut editors = self.resource_editors(scheme, &path, is_diff);
        let (mut default_id, mut pattern) = self.default_editor(scheme, &path, true, is_diff);
        if !is_diff && self.split_settings_enabled(scheme, &path) {
            // The split settings editor ranks as the text editor and its glob is the longer one.
            if default_id == TEXT_EDITOR_ID && (default_id.clone(), pattern.clone()) == self.default_editor(scheme, &path, false, false) {
                (default_id, pattern) = (SPLIT_SETTINGS_EDITOR_ID.to_owned(), "**/settings.json".to_owned());
            }
            editors.insert(0, (SPLIT_SETTINGS_EDITOR_ID.to_owned(), "Split Settings Editor".to_owned(), "Built-in".to_owned(), 0, false));
        }
        let editors = available_editors(editors, &current, &hidden)?;
        Some(EditorTypes { resource, diff, current, editors, default_id, pattern })
    }

    /// preferencesContribution.ts: `**/settings.json` opens in the split settings editor while
    /// `workbench.settings.useSplitJSON` or `openDefaultSettings` is on.
    fn split_settings_enabled(&self, scheme: &str, path: &Path) -> bool {
        let enabled = self.setting("workbench.settings.useSplitJSON") == json!(true) || self.setting("workbench.settings.openDefaultSettings") == json!(true);
        enabled && super::custom_editors::resource_matcher(scheme, path).is_some_and(|matches| matches("**/settings.json"))
    }

    /// `.breadcrumbs-editor-type` (breadcrumbscontrol.css): the current type's label and a chevron, 11px, at the
    /// breadcrumbs' end; a click shows the editor type menu below it.
    pub(super) fn editor_type_picker(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let types = self.editor_type_control()?;
        let label = types.editors.iter().find(|(id, _, _)| *id == types.current).map_or(types.current.clone(), |(id, label, _)| types.label(id, label));
        let foreground = self.color("breadcrumb.foreground");
        let focus_foreground = self.color("breadcrumb.focusForeground");
        let (bounds, group) = (self.editor_type_picker.bounds.clone(), self.editors.id);
        let measure = canvas(move |element, _, _| _ = bounds.borrow_mut().insert(group, element), |_, _, _, _| ()).absolute().inset_0();
        let chevron = div()
            .ml(px(2.))
            .relative()
            .child(self.icon("chevron-down", 13., 13., foreground).group_hover("breadcrumbs-editor-type", |icon| icon.invisible()))
            .child(div().absolute().inset_0().invisible().group_hover("breadcrumbs-editor-type", |icon| icon.visible()).child(self.icon("chevron-down", 13., 13., focus_foreground)));
        Some(
            div()
                .id("breadcrumbs-editor-type")
                .group("breadcrumbs-editor-type")
                .relative()
                .flex_none()
                .ml_auto()
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
                .hover(move |picker| picker.text_color(focus_foreground))
                .child(measure)
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.clone()))
                .child(chevron)
                .child(self.hover_target("breadcrumbs-editor-type-hover", HoverContent::Text(format!("Editor: {label}")), HoverOptions::mouse()))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.show_editor_type_menu(window, cx);
                })),
        )
    }

    /// The active group's control as breadcrumbsControl.ts last computed it: anew once its active editor or the settings
    /// changed, else as it was, so an editor opened before the extensions registered their editors keeps none.
    fn editor_type_control(&self) -> Option<EditorTypes> {
        let key = self.editor_type_control_key();
        let mut controls = self.editor_type_controls.borrow_mut();
        if let Some((computed, types)) = controls.get(&self.editors.id)
            && *computed == key
        {
            return types.clone();
        }
        let types = self.available_editor_types();
        controls.insert(self.editors.id, (key, types.clone()));
        types
    }

    fn editor_type_control_key(&self) -> String {
        let active = self.editors.active().map(|editor| (editor.model_id, self.active_editor_type().map(|(_, current)| current), self.editors.active_diff().is_some()));
        format!("{active:?} {} {}", self.setting("breadcrumbs.showEditorType"), self.setting("workbench.editor.hiddenEditorTypes"))
    }

    /// The active group's editor opened before the extensions registered their editors (editorResolverService.ts):
    /// only the text editor matched, so its control is hidden until the active editor changes.
    pub(super) fn set_editor_type_control_before_registration(&self) {
        let key = self.editor_type_control_key();
        self.editor_type_controls.borrow_mut().insert(self.editors.id, (key, None));
    }

    /// `Editors::insert_moved` into the active group: the moved editor opens a new pane there, whose control
    /// breadcrumbsControl.ts computes anew.
    pub(super) fn insert_moved_editor(&mut self, editor: crate::editor::OpenEditor) {
        self.editor_type_controls.borrow_mut().remove(&self.editors.id);
        self.editors.insert_moved(editor);
    }

    /// Every group's restored editor, also opened before the extensions registered their editors.
    pub fn set_restored_editor_type_controls(&mut self) {
        for group in self.groups_in_grid_order() {
            self.with_group(group, |workbench| workbench.set_editor_type_control_before_registration());
        }
    }

    /// `createEditorTypeActions`: one action per editor (the current one checked) that reopens with it, with Alt opens
    /// it to the side, then the "Set Default" submenu (the effective default checked).
    fn show_editor_type_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(types) = self.available_editor_types() else { return };
        let diff = types.diff.is_some();
        let mut reopen = Vec::new();
        let mut set_default = Vec::new();
        for (id, label, provider) in &types.editors {
            let title = label_with_source(&types.label(id, label), provider);
            let reopen_id = format!("{REOPEN_PREFIX}{id}");
            let editor_id = id.clone();
            self.commands.register(&reopen_id, move |workbench, _, window, cx| match workbench.available_editor_types() {
                Some(types) => workbench.reopen_editor_type(&types, &editor_id, window, cx),
                None => Outcome::done(),
            });
            // The current editor of a singleton input (a notebook or custom editor) cannot open twice.
            let singleton = types.current != TEXT_EDITOR_ID && types.current != SPLIT_SETTINGS_EDITOR_ID;
            let alt = (*id != types.current || !singleton).then(|| {
                let side_id = format!("{OPEN_TO_SIDE_PREFIX}{id}");
                let (editor_id, side_resource) = (id.clone(), types.resource.clone());
                self.commands.register(&side_id, move |workbench, _, window, cx| workbench.open_editor_type_to_side(&side_resource, &editor_id, window, cx));
                (side_id, format!("{title} (Open to the Side)"))
            });
            reopen.push(MenuAction::Command { id: reopen_id, title: title.clone(), enabled: true, checked: *id == types.current, alt });
            let default_command = format!("{SET_DEFAULT_PREFIX}{id}");
            let editor_id = id.clone();
            let default_resource = types.resource.clone();
            self.commands.register(&default_command, move |workbench, _, window, cx| {
                workbench.set_default_editor(&default_resource, &editor_id, diff);
                match workbench.available_editor_types() {
                    Some(types) => workbench.reopen_editor_type(&types, &editor_id, window, cx),
                    None => Outcome::done(),
                }
            });
            set_default.push(MenuAction::Command { id: default_command, title, enabled: true, checked: *id == types.default_id, alt: None });
        }
        let mut actions = reopen;
        actions.push(MenuAction::Separator);
        let submenu_title = match diff {
            true => format!("Set Default (Diff Only) for '{}'", types.pattern),
            false => format!("Set Default for '{}'", types.pattern),
        };
        actions.push(MenuAction::Submenu { title: submenu_title, actions: set_default, checked: false });
        if let Some((original, modified)) = types.diff {
            actions.push(MenuAction::Separator);
            for (command, title, side) in [(OPEN_ORIGINAL, "Open Original", original), (OPEN_MODIFIED, "Open Modified", modified)] {
                self.commands.register(command, move |workbench, _, window, cx| workbench.open_with_resolver(&side, None, json!({}), window, cx));
                actions.push(MenuAction::Command { id: command.to_owned(), title: title.to_owned(), enabled: true, checked: false, alt: None });
            }
        }
        let anchor = self.editor_type_picker.bounds.borrow().get(&self.editors.id).copied().unwrap_or_default();
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// editorCommands.ts `reopenActiveEditorWith`: the split settings editor turns into its settings file's text
    /// editor and back; any other type reopens through the resolver.
    pub(super) fn reopen_editor_type(&mut self, types: &EditorTypes, editor_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if types.current == editor_id {
            return Outcome::done();
        }
        let mut current = types.current.clone();
        if current == SPLIT_SETTINGS_EDITOR_ID {
            let entry = self.editors.active_tab().expect("the side by side editor is active").model_id;
            self.editors.remove_side_by_side(entry);
            self.editors_changed(cx);
            current = TEXT_EDITOR_ID.to_owned();
        }
        if editor_id == SPLIT_SETTINGS_EDITOR_ID {
            return self.show_split_settings(&types.resource, window, cx);
        }
        self.reopen_with(&types.resource, &current, editor_id, false, window, cx)
    }

    /// The Alt action: the resource opens with the editor in the group to the side, the source stays.
    fn open_editor_type_to_side(&mut self, resource: &Value, editor_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.activate_column(&json!(-2));
        let opened_with = if editor_id == SPLIT_SETTINGS_EDITOR_ID { TEXT_EDITOR_ID } else { editor_id };
        let outcome = self.open_with_resolver(resource, Some(opened_with), json!({ "pinned": true, "preserveFocus": false }), window, cx);
        match editor_id == SPLIT_SETTINGS_EDITOR_ID {
            true => self.show_split_settings(resource, window, cx),
            false => outcome,
        }
    }

    /// The active settings file's text editor becomes the split settings editor.
    fn show_split_settings(&mut self, resource: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Ok((_, path)) = uri_parts(resource) else { return Outcome::done() };
        if let Some(secondary) = self.split_settings_secondary(&path) {
            self.show_side_by_side(Path::new(secondary), window, cx);
        }
        Outcome::done()
    }
}
