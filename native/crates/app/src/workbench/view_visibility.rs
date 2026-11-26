//! View visibility (viewContainerModel.ts `ViewDescriptorsState`, viewDescriptorService.ts
//! `registerViewsVisibilityActionsForContainer`): a view that `canToggleVisibility` is hidden while its container's
//! `<storage id>.hidden` profile state says so, else while it is `hideByDefault` (`"visibility": "hidden"` in
//! `contributes.views`). `<view id>.toggleVisibility` shows or hides it from the container's Views submenu.
// Not ported yet: the ViewContainerTitleContext and ViewTitleContext menus (right click on the title and on pane
// headers) with `<view id>.removeView`, workspace-scoped visibility (`visibleWorkspace`).

use gpui::{Context, Div, MouseButton, MouseDownEvent, Stateful, Window, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::view_panes::{EMPTY_VIEW, FILE_VIEW, OUTLINE_VIEW, TIMELINE_VIEW};
use crate::commands::Outcome;
use crate::menus::MenuAction;

#[cfg(test)]
mod tests;

/// A view of a container as its Views submenu lists it.
struct ViewEntry {
    id: &'static str,
    name: String,
    can_toggle: bool,
}

/// `viewContainer.storageId || `${viewContainer.id}.state``.
pub(super) fn container_storage_id(container: &str) -> String {
    match container {
        "explorer" => "workbench.explorer.views.state".into(),
        "scm" => "workbench.scm.views.state".into(),
        "debug" => "workbench.view.debug.state".into(),
        "test" => "workbench.view.extension.test.state".into(),
        other => format!("workbench.view.extension.{other}.state"),
    }
}

impl Workbench {
    /// `isViewDescriptorVisible`: the stored `isHidden`, else `hideByDefault`.
    pub(super) fn view_hidden(&self, container: &str, id: &str) -> bool {
        let entries = self.views_visibility_state(container);
        match entries.iter().find(|entry| entry["id"] == id).and_then(|entry| entry["isHidden"].as_bool()) {
            Some(hidden) => hidden,
            None => self.tree_views.views.iter().any(|view| view.id == id && view.hide_by_default),
        }
    }

    /// The container's `.hidden` profile state: `{ id, isHidden, order? }` per view it has seen.
    pub(super) fn views_visibility_state(&self, container: &str) -> Vec<Value> {
        let key = format!("{}.hidden", container_storage_id(container));
        let stored = self.profile_storage.lock().unwrap().get(&key).and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok());
        stored.unwrap_or_default()
    }

    /// `toggleViewVisibility`: the view's entry flips, the others stay.
    fn toggle_view_visibility(&mut self, container: &str, id: &str, cx: &mut Context<Self>) {
        let hidden = !self.view_hidden(container, id);
        let mut entries = self.views_visibility_state(container);
        match entries.iter_mut().find(|entry| entry["id"] == id) {
            Some(entry) => entry["isHidden"] = json!(hidden),
            None => entries.push(json!({ "id": id, "isHidden": hidden })),
        }
        let key = format!("{}.hidden", container_storage_id(container));
        self.profile_storage.lock().unwrap().set(&key, Value::Array(entries).to_string());
        cx.notify();
    }

    /// `activeViewDescriptors` of the container in their order, hidden ones included.
    fn container_view_entries(&self, container: &str) -> Vec<ViewEntry> {
        let mut entries = Vec::new();
        if container == "explorer" {
            match self.workspace().is_some_and(|workspace| !workspace.folders.is_empty()) {
                true => entries.push(ViewEntry { id: FILE_VIEW, name: "Folders".into(), can_toggle: false }),
                false => entries.push(ViewEntry { id: EMPTY_VIEW, name: "No Folder Opened".into(), can_toggle: true }),
            }
            entries.push(ViewEntry { id: OUTLINE_VIEW, name: "Outline".into(), can_toggle: true });
            entries.push(ViewEntry { id: TIMELINE_VIEW, name: "Timeline".into(), can_toggle: true });
        }
        let contributed = self.tree_views.views.iter().filter(|view| view.container == container && view.when.as_ref().is_none_or(|when| when.evaluate(&self.context_keys)));
        entries.extend(contributed.map(|view| ViewEntry { id: view.id, name: view.name.clone(), can_toggle: true }));
        entries
    }

    /// The `ViewsSubMenu` toggles: a view's toggle is disabled while it is the container's last visible one.
    fn views_submenu(&mut self, container: &'static str) -> Vec<MenuAction> {
        let entries = self.container_view_entries(container);
        let shown: Vec<bool> = entries.iter().map(|entry| !entry.can_toggle || !self.view_hidden(container, entry.id)).collect();
        let visible_count = shown.iter().filter(|shown| **shown).count();
        let mut actions = Vec::new();
        for (entry, &checked) in entries.iter().zip(&shown) {
            let command = format!("{}.toggleVisibility", entry.id);
            if self.commands.get(&command).is_none() {
                let id = entry.id;
                self.commands.register(command.clone(), move |workbench, _, _, cx| {
                    workbench.toggle_view_visibility(container, id, cx);
                    Outcome::done()
                });
            }
            let enabled = entry.can_toggle && (!checked || visible_count > 1);
            actions.push(MenuAction::Command { id: command, title: entry.name.clone(), enabled, checked, alt: None });
        }
        actions
    }

    /// The container title's "..." (`MenuId.ViewContainerTitle`): the Views submenu.
    pub(super) fn view_container_more_actions(&self, container: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        self.icon("ellipsis", 22., 16., self.foreground_or("icon.foreground"))
            .id(gpui::SharedString::from(format!("{container}-more")))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|action| action.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window: &mut Window, cx| {
                cx.stop_propagation();
                let views = workbench.views_submenu(container);
                let actions = vec![MenuAction::Submenu { title: "Views".into(), actions: views, checked: false }];
                let anchor = gpui::Bounds::new(event.position, gpui::size(px(0.), px(0.)));
                workbench.show_context_menu_actions(actions, &Default::default(), anchor, Vec::new(), window, cx);
            }))
    }
}

/// `convertInitialVisibility`: `(hideByDefault, collapsed)` of a `contributes.views` entry.
pub(super) fn initial_visibility(view: &Value) -> (bool, bool) {
    match view["visibility"].as_str() {
        Some("hidden") => (true, false),
        Some("collapsed") => (false, true),
        _ => (false, false),
    }
}
