//! Locked editor groups (editorGroupModel.ts `lock`, editorCommands.ts `setEditorGroupLock`, editorGroupFinder.ts,
//! editorGroupView.ts `openEditor`): a locked group takes editors only when a gesture picks it, such as a drop.

use std::path::Path;

use gpui::{AnyElement, Context, Div, div, prelude::*, px};
use serde_json::json;

use super::super::Workbench;
use super::super::custom_editors::TEXT_EDITOR_ID;
use super::super::hover::HoverOptions;
use crate::commands::Outcome;
use crate::editor::OpenEditor;

impl Workbench {
    pub(in crate::workbench) fn register_locked_group_commands(&mut self) {
        let commands = [
            ("workbench.action.toggleEditorGroupLock", None),
            ("workbench.action.lockEditorGroup", Some(true)),
            ("workbench.action.unlockEditorGroup", Some(false)),
        ];
        for (id, locked) in commands {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.leave_panel_group();
                let group = workbench.editors.id;
                let locked = locked.unwrap_or(!workbench.editors.locked.contains(&group));
                match locked {
                    true => workbench.editors.locked.insert(group),
                    false => workbench.editors.locked.remove(&group),
                };
                workbench.editors_changed(cx);
                Outcome::done()
            });
        }
    }

    /// editorGroupFinder.ts: an editor opening in the active group while it is locked, and does not have the editor
    /// open, opens in the most recent unlocked group instead, else in a new group beside it.
    pub(in crate::workbench) fn leave_locked_group(&mut self, scheme: &str, path: &Path) {
        let active = self.editors.id;
        if !self.editors.locked.contains(&active) || self.editors.editors.iter().any(|editor| editor.document().is(scheme, path)) {
            return;
        }
        let unlocked = self.editors.group_ids_mru().into_iter().find(|group| self.part_of_group(*group).is_some() && !self.editors.locked.contains(group));
        let target = unlocked.unwrap_or_else(|| self.add_group_beside(self.side_by_side_direction()));
        self.activate_group(target);
    }

    /// editorGroupView.ts `openEditor`: a group whose first editor opened while there are other groups locks when
    /// `workbench.editor.autoLockGroups` lists the editor's id.
    pub(in crate::workbench) fn auto_lock_group(&mut self) {
        let Some(group) = self.editors.opened_first_editor.take() else { return };
        if !self.editors.has_several_groups() || self.part_of_group(group).is_none() {
            return;
        }
        let (editors, _) = self.editors.group_tabs(group);
        let [editor] = editors else { return };
        let id = self.editor_type_id(editor);
        if self.setting("workbench.editor.autoLockGroups").get(&id) == Some(&json!(true)) {
            self.editors.locked.insert(group);
        }
    }

    /// `EditorInput.editorId`: a webview's `mainThreadWebview-<viewType>`, a custom editor's or a notebook's type,
    /// else the text editor's.
    fn editor_type_id(&self, editor: &OpenEditor) -> String {
        let path = &editor.document().path;
        if let Some(tab) = self.host.webview_tabs.get(path) {
            return tab.view_type.clone();
        }
        if editor.document().scheme == crate::notebook::NOTEBOOK_EDITOR_SCHEME
            && let Some(model) = self.host.notebooks.model(&path.to_string_lossy())
        {
            return model.view_type.clone();
        }
        TEXT_EDITOR_ID.to_owned()
    }

    /// editor.contribution.ts: Unlock Group (`Codicon.lock`, toggled) in a locked group's title, Lock Group
    /// (`Codicon.unlock`) in an editor window's, left of where Close sits.
    pub(in crate::workbench) fn lock_group_action(&self, group: u32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let in_editor_window = self.part_of_group(group).is_some_and(|part| part != super::MAIN_PART);
        match self.editors.locked.contains(&group) {
            true => Some(self.group_action("lock", "Unlock Group", "workbench.action.unlockEditorGroup", true, cx)),
            false if in_editor_window => Some(self.group_action("unlock", "Lock Group", "workbench.action.lockEditorGroup", false, cx)),
            false => None,
        }
    }

    /// editorGroupView.ts `createContainerToolbar` (`MenuId.EmptyEditorGroup`): an empty group shows Lock or Unlock
    /// Group and Close Group when it is locked, in an editor window or one of several groups (editorgroupview.css).
    pub(in crate::workbench) fn empty_group_toolbar(&self, group: u32, cx: &mut Context<Self>) -> Option<Div> {
        let part = self.part_of_group(group)?;
        let closable = part != super::MAIN_PART || self.part_groups(part).len() > 1;
        if !closable && !self.editors.locked.contains(&group) {
            return None;
        }
        let close = closable.then(|| self.group_action("close", "Close Group", "workbench.action.closeGroup", false, cx));
        let actions = self.lock_group_action(group, cx).into_iter().chain(close);
        Some(div().flex_none().h(px(35.)).flex().items_center().justify_end().children(actions.map(|action| div().mr(px(4.)).child(action))))
    }

    fn group_action(&self, icon: &str, title: &'static str, command: &'static str, toggled: bool, cx: &mut Context<Self>) -> AnyElement {
        let toggled_background = self.theme.color("actionBar.toggledBackground").unwrap_or(self.color("inputOption.activeBackground"));
        self.icon(icon, 22., 16., self.color("icon.foreground"))
            .id(command)
            .rounded(px(5.))
            .cursor_pointer()
            .when(toggled, |button| button.bg(toggled_background))
            .when(!toggled, |button| button.hover(|button| button.bg(self.color("toolbar.hoverBackground"))))
            .on_click(cx.listener(move |workbench, _, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx))))
            .child(self.action_hover(title, command, HoverOptions::instant()))
            .into_any_element()
    }
}
