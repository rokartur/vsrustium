//! outlineActions.ts and outlineViewState.ts: the Outline view's title actions (Collapse All or Expand All, then
//! Follow Cursor, Filter on Type and Sort By under More Actions...) and their state in workspace storage.

use std::collections::HashMap;

use gpui::{Bounds, Context, Div, MouseButton, MouseDownEvent, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::outline_pane::Status;
use crate::commands::Outcome;
use crate::contextkey::Expr;
use crate::menus::{Entry, MenuAction, MenuId, MenuItem, Toggled, menu_actions};

/// outlineViewState.ts `persist`: `outline/state` in workspace storage.
const STATE_KEY: &str = "outline/state";
/// outline.ts `IOutlinePane.Id`.
const OUTLINE_VIEW: &str = "outline";

/// outline.ts `OutlineSortOrder`, stored as its number.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum SortOrder {
    #[default]
    ByPosition,
    ByName,
    ByKind,
}

pub struct OutlineViewState {
    pub follow_cursor: bool,
    pub filter_on_type: bool,
    pub sort_by: SortOrder,
}

impl Default for OutlineViewState {
    fn default() -> Self {
        Self { follow_cursor: false, filter_on_type: true, sort_by: SortOrder::ByPosition }
    }
}

impl Workbench {
    pub(super) fn register_outline_actions(&mut self) {
        self.commands.register("outline.collapse", |workbench, _, _, cx| {
            workbench.set_outline_all_expanded(false, cx);
            Outcome::done()
        });
        self.commands.register("outline.expand", |workbench, _, _, cx| {
            workbench.set_outline_all_expanded(true, cx);
            Outcome::done()
        });
        self.commands.register("outline.followCursor", |workbench, _, _, cx| {
            workbench.outline_pane.view_state.follow_cursor = !workbench.outline_pane.view_state.follow_cursor;
            workbench.outline_view_state_changed(cx);
            workbench.reveal_outline_active_element(true, cx);
            Outcome::done()
        });
        self.commands.register("outline.filterOnType", |workbench, _, _, cx| {
            workbench.outline_pane.view_state.filter_on_type = !workbench.outline_pane.view_state.filter_on_type;
            workbench.outline_view_state_changed(cx);
            Outcome::done()
        });
        for (id, order) in [("outline.sortByPosition", SortOrder::ByPosition), ("outline.sortByName", SortOrder::ByName), ("outline.sortByKind", SortOrder::ByKind)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.outline_pane.view_state.sort_by = order;
                workbench.outline_view_state_changed(cx);
                Outcome::done()
            });
        }
    }

    /// outlineViewState.ts `restore`.
    pub(super) fn restore_outline_state(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(STATE_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        let Some(stored) = stored else { return };
        let state = &mut self.outline_pane.view_state;
        state.follow_cursor = stored["followCursor"] == true;
        state.sort_by = match stored["sortBy"].as_u64() {
            Some(1) => SortOrder::ByName,
            Some(2) => SortOrder::ByKind,
            _ => SortOrder::ByPosition,
        };
        if let Some(filter_on_type) = stored["filterOnType"].as_bool() {
            state.filter_on_type = filter_on_type;
        }
    }

    /// outlineViewState.ts `persist` on every change.
    fn outline_view_state_changed(&mut self, cx: &mut Context<Self>) {
        let state = &self.outline_pane.view_state;
        let stored = json!({ "followCursor": state.follow_cursor, "sortBy": state.sort_by as u8, "filterOnType": state.filter_on_type });
        self.workspace_storage.lock().unwrap().set(STATE_KEY, stored.to_string());
        cx.notify();
    }

    /// `tree.collapseAll` and `tree.expandAll` over every row with children, the hidden ones included.
    fn set_outline_all_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let Status::Ready { model_id, outline, .. } = &self.outline_pane.status else { return };
        let model_id = *model_id;
        let mut ids: Vec<String> = outline.groups.iter().map(|group| group.id.clone()).collect();
        let mut stack: Vec<&crate::outline_model::OutlineElement> = outline.groups.iter().flat_map(|group| &group.children).collect();
        while let Some(element) = stack.pop() {
            if !element.children.is_empty() {
                ids.push(element.id.clone());
            }
            stack.extend(&element.children);
        }
        for id in ids {
            self.set_outline_row_expanded(model_id, &id, expanded);
        }
        cx.notify();
    }

    /// `outlineAllCollapsed`: every top-level row that can collapse is collapsed.
    pub(super) fn outline_all_collapsed(&self) -> bool {
        let Status::Ready { model_id, outline, .. } = &self.outline_pane.status else { return false };
        self.outline_rows(*model_id, outline).iter().filter(|row| row.depth == 0 && row.has_children).all(|row| !self.outline_row_expanded(*model_id, row.id))
    }

    /// The `view/title` items of outlineActions.ts.
    fn outline_menus(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let mut add = |id: &str, title: &str, group: &str, order, when: &str, toggled: Option<&str>| {
            let entry = Entry::Command { id: id.to_string(), precondition: None, toggled: Toggled::when(toggled.and_then(Expr::deserialize)), alt: None };
            items.push(MenuItem { menu: MenuId::ViewTitle, entry, title: title.to_string(), group: group.to_string(), order, when: Expr::deserialize(&format!("view == {OUTLINE_VIEW}{when}")) });
        };
        add("outline.collapse", "Collapse All", "navigation", 0., " && outlineAllCollapsed == false", None);
        add("outline.expand", "Expand All", "navigation", 0., " && outlineAllCollapsed", None);
        add("outline.followCursor", "Follow Cursor", "config", 1., "", Some("outlineFollowsCursor"));
        add("outline.filterOnType", "Filter on Type", "config", 2., "", Some("outlineFiltersOnType"));
        add("outline.sortByPosition", "Sort By: Position", "sort", 1., "", Some("outlineSortMode == 0"));
        add("outline.sortByName", "Sort By: Name", "sort", 2., "", Some("outlineSortMode == 1"));
        add("outline.sortByKind", "Sort By: Category", "sort", 3., "", Some("outlineSortMode == 2"));
        items
    }

    fn outline_menu_context(&self, window: &Window) -> HashMap<String, Value> {
        let state = &self.outline_pane.view_state;
        let mut context = self.key_context(window);
        context.insert("view".into(), json!(OUTLINE_VIEW));
        context.insert("outlineFollowsCursor".into(), json!(state.follow_cursor));
        context.insert("outlineFiltersOnType".into(), json!(state.filter_on_type));
        context.insert("outlineSortMode".into(), json!(state.sort_by as u8));
        context.insert("outlineAllCollapsed".into(), json!(self.outline_all_collapsed()));
        context
    }

    /// The pane's title toolbar: the navigation actions as icons, the rest under More Actions....
    pub(super) fn outline_title_actions(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let menus = self.outline_menus();
        let context = self.outline_menu_context(window);
        let navigation: Vec<MenuItem> = menus.iter().filter(|item| item.group == "navigation").cloned().collect();
        let secondary: Vec<MenuItem> = menus.iter().filter(|item| item.group != "navigation").cloned().collect();
        let mut bar = div().flex().pr(px(8.));
        for action in menu_actions(&navigation, MenuId::ViewTitle, &context) {
            let MenuAction::Command { id, title, .. } = action else { continue };
            let icon = if id == "outline.collapse" { "collapse-all" } else { "expand-all" };
            bar = bar.child(self.outline_action_button(icon, title).on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                drop(workbench.execute_command(&id, Vec::new(), window, cx));
            })));
        }
        let secondary_actions = menu_actions(&secondary, MenuId::ViewTitle, &context);
        bar.child(self.outline_action_button("ellipsis", "More Actions...".into()).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_context_menu_actions(secondary_actions.clone(), &context, Bounds::new(event.position, Default::default()), Vec::new(), window, cx);
            }),
        ))
    }

    fn outline_action_button(&self, icon: &'static str, title: String) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        self.icon(icon, 22., 16., foreground)
            .id(gpui::SharedString::from(format!("outline-action-{icon}")))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|action| action.bg(self.color("toolbar.hoverBackground")))
            .child(self.hover_target(gpui::SharedString::from(format!("outline-action-hover-{icon}")), HoverContent::Text(title), HoverOptions::action_item()))
    }
}
