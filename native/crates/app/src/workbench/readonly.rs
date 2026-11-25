//! filesConfigurationService.ts `isReadonly` with its session overrides (fileActions.ts
//! `BaseSetActiveEditorReadonlyInSession`), and readOnlyMessage/contribution.ts: an edit attempted in a read-only
//! editor shows why it is read-only.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use super::editor_message::MessagePart;
use crate::commands::Outcome;

#[cfg(test)]
mod tests;

/// Why a file is read-only (`READONLY_MESSAGES`); `vscode` is the read-only provider of settingsFilesystemProvider.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readonly {
    Provider,
    Session,
    Configured,
    Locked,
    /// The editor itself is read-only (`readOnly` without `readOnlyMessage`): the Output view.
    Editor,
}

const SET_WRITEABLE: &str = "workbench.action.files.setActiveEditorWriteableInSession";
const TOGGLE: &str = "workbench.action.files.toggleActiveEditorReadonlyInSession";

impl Readonly {
    /// The `readOnlyMessage` markdown, its `command:` links as parts.
    pub fn message(self) -> Vec<MessagePart> {
        match self {
            Readonly::Provider => vec![MessagePart::text("Editor is read-only because the file system of the file is read-only.")],
            Readonly::Session => vec![
                MessagePart::text("Editor is read-only because the file was set read-only in this session. "),
                MessagePart::link("Click here", SET_WRITEABLE, vec![]),
                MessagePart::text(" to set writeable."),
            ],
            Readonly::Configured => vec![
                MessagePart::text("Editor is read-only because the file was set read-only via settings. "),
                MessagePart::link("Click here", "workbench.action.openSettings", vec![json!("files.readonly")]),
                MessagePart::text(" to configure or "),
                MessagePart::link("toggle for this session", TOGGLE, vec![]),
                MessagePart::text("."),
            ],
            Readonly::Locked => vec![
                MessagePart::text("Editor is read-only because of file permissions. "),
                MessagePart::link("Click here", SET_WRITEABLE, vec![]),
                MessagePart::text(" to set writeable anyway."),
            ],
            Readonly::Editor => vec![MessagePart::text("Cannot edit in read-only editor")],
        }
    }
}

/// `sessionReadonlyOverrides`: by scheme and path, `true` read-only and `false` writeable.
pub(super) type SessionOverrides = HashMap<(String, PathBuf), bool>;

impl Workbench {
    pub(super) fn register_readonly_commands(&mut self) {
        for (id, state) in [
            ("workbench.action.files.setActiveEditorReadonlyInSession", Some(Some(true))),
            (SET_WRITEABLE, Some(Some(false))),
            (TOGGLE, None),
            ("workbench.action.files.resetActiveEditorReadonlyInSession", Some(None)),
        ] {
            self.commands.register(id, move |workbench, _, _, cx| workbench.update_active_editor_readonly(state, cx));
        }
    }

    /// `updateReadonly` of the active editor's file: `Some` sets the override (`None` resets it), `None` toggles
    /// from the state it has now.
    fn update_active_editor_readonly(&mut self, state: Option<Option<bool>>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active().filter(|editor| editor.document.scheme != "untitled") else { return Outcome::done() };
        let resource = (editor.document.scheme.clone(), editor.document.path.clone());
        let state = state.unwrap_or_else(|| Some(self.readonly(&resource.0, &resource.1).is_none()));
        match state {
            Some(readonly) => self.readonly_overrides.insert(resource, readonly),
            None => self.readonly_overrides.remove(&resource),
        };
        cx.notify();
        Outcome::done()
    }

    /// `isReadonly`: a read-only file system provider wins over all; then the session override; the user data and the workspace file are never read-only; then
    /// `files.readonlyInclude` minus `files.readonlyExclude`; then a file without the owner's write permission
    /// under `files.readonlyFromPermissions`. Untitled models are never read-only.
    pub(super) fn readonly(&self, scheme: &str, path: &Path) -> Option<Readonly> {
        if scheme == "untitled" {
            return None;
        }
        if scheme == crate::output::OUTPUT_SCHEME {
            return Some(Readonly::Editor);
        }
        if scheme == "vscode" {
            return Some(Readonly::Provider);
        }
        if let Some(&readonly) = self.readonly_overrides.get(&(scheme.to_owned(), path.to_path_buf())) {
            return readonly.then_some(Readonly::Session);
        }
        let settings = self.settings.lock().unwrap();
        let workspace_file = settings.workspace.as_ref().and_then(|workspace| workspace.file.as_deref());
        if path.starts_with(settings.user_home()) || workspace_file == Some(path) {
            return None;
        }
        drop(settings);
        let expression = |key: &'static str| move |setting: search::query::Setting, _: Option<&Path>| setting(Path::new(""), key).as_object().cloned().unwrap_or_default();
        if self.resource_glob_matcher(&expression("files.readonlyInclude"))(path) {
            return (!self.resource_glob_matcher(&expression("files.readonlyExclude"))(path)).then_some(Readonly::Configured);
        }
        let locked = || std::fs::metadata(path).is_ok_and(|stat| stat.permissions().mode() & 0o200 == 0);
        (scheme == "file" && self.setting("files.readonlyFromPermissions") == Value::Bool(true) && locked()).then_some(Readonly::Locked)
    }

    pub(super) fn active_editor_readonly(&self) -> Option<Readonly> {
        let editor = self.editors.active()?;
        self.readonly(&editor.document.scheme, &editor.document.path)
    }

    /// Whether an edit of the active editor must not run because it is read-only. An edit command's
    /// `precondition: EditorContextKeys.writable` fails silently; a cursor edit (viewModelImpl.ts
    /// `_executeCursorEdit`: typing, paste, cut, deleteLeft, deleteRight) fires `onDidAttemptReadOnlyEdit`, which
    /// shows the editor's read-only message.
    pub(super) fn edit_blocked(&mut self, cursor_edit: bool, cx: &mut Context<Self>) -> bool {
        let Some(readonly) = self.active_editor_readonly() else { return false };
        if cursor_edit {
            self.show_editor_message(readonly.message(), cx);
        }
        true
    }
}
