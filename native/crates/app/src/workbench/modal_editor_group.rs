//! The modal editor part (modalEditorPart.ts) for text editors: preferencesService.ts opens the settings and
//! keybindings JSON files in `MODAL_GROUP` unless `workbench.editor.useModal` is `off`. The group lives outside the grid
//! like the panel's; the part shows its active editor without tabs, breadcrumbs or editor actions (`showTabs: none`,
//! `editorActionsLocation: hidden`) under the header with the editor's label, and closes with its last editor.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::keybindings_editor_view::ModalHeader;
use crate::editor::MODAL_GROUP;
use crate::theme::ThemeKind;

#[cfg(test)]
mod tests;

impl Workbench {
    /// `getEditorGroupFromOptions`: the modal group unless `workbench.editor.useModal` is `off` or extension tests run.
    pub(super) fn open_preferences_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.setting("workbench.editor.useModal") == json!("off") || self.skip_dialogs {
            return self.open_file(path, true, window, cx);
        }
        // shortcut: the Settings and Keyboard Shortcuts editors are not editors of the modal group, so the file
        // replaces them where upstream keeps them as the group's other editors; upgrade with the editor pane registry.
        self.close_keybindings_editor(window, cx);
        self.close_settings_editor(window, cx);
        self.close_profiles_editor(cx);
        self.editors.ensure_detached_group(MODAL_GROUP);
        self.activate_group(MODAL_GROUP);
        self.open_file_in_active_group(path, true, window, cx);
    }

    pub(super) fn modal_group_open(&self) -> bool {
        let (editors, _) = self.editors.group_tabs(MODAL_GROUP);
        (self.editors.id == MODAL_GROUP || self.editors.group(MODAL_GROUP).is_some()) && !editors.is_empty()
    }

    pub(super) fn modal_group_focused(&self, window: &Window) -> bool {
        self.editors.id == MODAL_GROUP && self.editor_focus.is_focused(window)
    }

    /// `ModalEditorPart.close`: its editors close, asking to save dirty ones; the group goes with the last.
    pub(super) fn close_modal_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.modal_group_open() {
            return;
        }
        self.activate_group(MODAL_GROUP);
        let all = (0..self.editors.editors.len()).collect();
        drop(self.close_editors(all, window, cx));
    }

    /// The modal context keys while focus is in the modal group's editor.
    pub(super) fn modal_group_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        if !self.modal_group_focused(window) {
            return;
        }
        context.insert("editorPartModal".into(), json!(true));
        context.insert("editorPartModalVisible".into(), json!(true));
        context.insert("editorPartModalMaximized".into(), json!(self.keybindings_editor.maximized));
    }

    /// The modal part around the group's active editor.
    pub(super) fn modal_group_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.modal_group_open() {
            return None;
        }
        let (resource, json_title, json_command) = self.with_group(MODAL_GROUP, |workbench| {
            let editor = workbench.editors.active().expect("an open modal group has an active editor");
            let path = editor.document().path.clone();
            let (json_title, json_command) = workbench.modal_editor_title_action(&path);
            (workbench.modal_resource_label(editor), json_title, json_command)
        });
        let header = ModalHeader { icon: "", title: "", json_title, json_command, resource: Some(resource) };
        let focus = self.editor_focus.clone();
        Some(self.modal_editor_part(header, &focus, window, cx, |workbench, _, _, window, cx| {
            let body = |workbench: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                let secondary = workbench.side_by_side_secondary(window, cx);
                let editor = workbench.editors.active().expect("an open modal group has an active editor");
                // `NoEditorTabsControl` keeps the breadcrumbs (breadcrumbs.enabled) without the tabs.
                let breadcrumbs = (workbench.setting("breadcrumbs.enabled") != json!(false)).then(|| workbench.breadcrumbs(editor));
                let part = div().flex_1().min_h_0().flex().flex_col().children(breadcrumbs);
                match secondary {
                    Some(secondary) => part.child(workbench.side_by_side_body(secondary, editor, window, cx)),
                    None => part.children(workbench.unicode_banner(editor, cx)).child(workbench.text_area(editor, window, cx)),
                }
            };
            match workbench.editors.id == MODAL_GROUP {
                true => body(workbench, window, cx).into_any_element(),
                false => workbench.swapped_group_body(MODAL_GROUP, body, window, cx).into_any_element(),
            }
        }))
    }

    /// `ModalEditorEditorTitle`: preferences.contribution.ts's Open Settings (UI) on a settings file and Open Keyboard
    /// Shortcuts on keybindings.json.
    fn modal_editor_title_action(&self, path: &std::path::Path) -> (&'static str, &'static str) {
        if self.keybindings.user_file.as_deref() == Some(path) {
            return ("Open Keyboard Shortcuts", "workbench.action.openGlobalKeybindings");
        }
        match self.is_user_settings_file(path) {
            true => ("Open Settings (UI)", "workbench.action.openSettings2"),
            false => ("", ""),
        }
    }

    /// `openUserSettingsEditorWhen`: the current or the default profile's settings.json.
    pub(super) fn is_user_settings_file(&self, path: &std::path::Path) -> bool {
        let settings = self.settings.lock().unwrap();
        let files = settings.user_files();
        files.settings == path || files.default_settings.as_deref() == Some(path)
    }

    /// The header's `ResourceLabel`: file icon, name and the folder as the medium description.
    fn modal_resource_label(&self, editor: &crate::editor::OpenEditor) -> AnyElement {
        let foreground = self.foreground_or("titleBar.activeForeground");
        let path = &editor.document().path;
        // sideBySideEditorInput.ts `getDescription`: none for two different sides.
        let folder = path.parent().map(|folder| crate::quick_input::path_label(folder, self.workspace())).filter(|label| !label.is_empty() && self.editors.side_group().is_none());
        // iconlabel.css `.label-description`: 0.9em at 0.7 opacity (0.95 in light themes), 0.5em after the name.
        let opacity = if self.theme.kind == ThemeKind::Light { 0.95 } else { 0.7 };
        div()
            .flex()
            .items_center()
            .min_w_0()
            .whitespace_nowrap()
            .children(self.document_icon(editor, 22., foreground).map(|icon| icon.mr(px(6.))))
            .child(self.editor_name(editor))
            .children(folder.map(|folder| div().ml(px(6.)).text_size(px(10.8)).opacity(opacity).truncate().child(folder)))
            .into_any_element()
    }
}
