//! The Source Control view's actions: the input history of scmService.ts `SCMInput` and `SCMInputHistory`, the
//! view title's "More Actions..." menu and View & Sort submenu of scmViewPane.ts, the focus and terminal commands of
//! scm.contribution.ts, and the Graph view's title and item actions of scmHistoryViewPane.ts.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{Bounds, Context, Div, MouseButton, MouseDownEvent, Rgba, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::search_view::SideBarView;
use crate::commands::Outcome;
use crate::contextkey::Expr;
use crate::menus::{CommandIcon, Entry, MenuAction, MenuId, MenuItem, Toggled, menu_actions};
use crate::scm::Provider;
use crate::scm_history::{HistoryItem, HistoryItemKind, SCM_INCOMING_HISTORY_ITEM_ID, SCM_OUTGOING_HISTORY_ITEM_ID};

const VIEW_PANE_ID: &str = "workbench.scm";
const HISTORY_VIEW_PANE_ID: &str = "workbench.scm.history";
const INPUT_HISTORY_KEY: &str = "scm.history";
/// `new HistoryNavigator2(history, 100)`.
const INPUT_HISTORY_CAPACITY: usize = 100;
const SHOW_STRUCTURAL_DIFF_KEY: &str = "scm.showStructuralDiff";
const GRAPH_VIEW_MODE_KEY: &str = "scm.graphView.viewMode";

/// base/common/history.ts `HistoryNavigator2<string>`: unique values, oldest first, the last one the draft, and a
/// cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryNavigator {
    values: Vec<String>,
    cursor: usize,
}

impl HistoryNavigator {
    pub fn new(history: &[String]) -> Self {
        let mut navigator = HistoryNavigator { values: vec![history.first().cloned().unwrap_or_default()], cursor: 0 };
        for value in history.iter().skip(1) {
            navigator.add(value.clone());
        }
        navigator
    }

    pub fn add(&mut self, value: String) {
        let existing = self.values.iter().position(|known| *known == value);
        self.values.push(value);
        if let Some(index) = existing {
            self.values.remove(index);
        }
        if self.values.len() > INPUT_HISTORY_CAPACITY {
            self.values.drain(..self.values.len() - INPUT_HISTORY_CAPACITY);
        }
        self.cursor = self.values.len() - 1;
    }

    /// `replaceLast`: the old last value.
    pub fn replace_last(&mut self, value: &str) -> String {
        let last = self.values.len() - 1;
        if self.values[last] == value {
            return value.to_string();
        }
        let old = std::mem::replace(&mut self.values[last], value.to_string());
        if let Some(index) = self.values[..last].iter().position(|known| known == value) {
            self.values.remove(index);
            if index < self.cursor {
                self.cursor -= 1;
            }
        }
        old
    }

    pub fn is_at_end(&self) -> bool {
        self.cursor == self.values.len() - 1
    }

    pub fn current(&self) -> &str {
        &self.values[self.cursor]
    }

    pub fn previous(&mut self) -> &str {
        self.cursor = self.cursor.saturating_sub(1);
        self.current()
    }

    pub fn next(&mut self) -> &str {
        self.cursor = (self.cursor + 1).min(self.values.len() - 1);
        self.current()
    }

    pub fn has(&self, value: &str) -> bool {
        self.values.iter().any(|known| known == value)
    }

    pub fn reset_cursor(&mut self) {
        self.cursor = self.values.len() - 1;
    }

    fn is_empty_draft(&self) -> bool {
        self.values.len() == 1 && self.values[0].is_empty()
    }
}

/// scmService.ts `SCMInputHistory`: the histories by provider label and root uri, stored as `scm.history` in
/// workspace storage; providers without a root uri keep theirs in memory.
#[derive(Default)]
pub struct InputHistories {
    loaded: bool,
    persisted: Vec<(String, Value, HistoryNavigator)>,
    in_memory: HashMap<u64, HistoryNavigator>,
}

/// `ResourceMap` keys: two uris are the same resource when scheme, authority and path match.
fn same_uri(a: &Value, b: &Value) -> bool {
    ["scheme", "authority", "path"].iter().all(|key| a[key].as_str().unwrap_or_default() == b[key].as_str().unwrap_or_default())
}

/// `rot`: `index` wrapped into `0..modulo`.
fn rot(index: isize, modulo: usize) -> usize {
    index.rem_euclid(modulo as isize) as usize
}

impl Workbench {
    pub(super) fn register_scm_actions(&mut self) {
        for (id, previous) in [("scm.viewNextCommit", false), ("scm.forceViewNextCommit", false), ("scm.viewPreviousCommit", true), ("scm.forceViewPreviousCommit", true)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.show_scm_history_value(previous, window, cx);
                Outcome::done()
            });
        }
        for (id, delta) in [("workbench.scm.action.focusNextInput", 1), ("workbench.scm.action.focusPreviousInput", -1)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.focus_scm_input_by(delta, window, cx);
                Outcome::done()
            });
        }
        for (id, delta) in [("workbench.scm.action.focusNextResourceGroup", 1), ("workbench.scm.action.focusPreviousResourceGroup", -1)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.focus_scm_resource_group(delta, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.scm.action.toggleShowStructuralDiff", |workbench, _, _, cx| {
            let show = !workbench.scm_show_structural_diff_stored();
            workbench.workspace_storage.lock().unwrap().set(SHOW_STRUCTURAL_DIFF_KEY, show.to_string());
            cx.notify();
            Outcome::done()
        });
        for (id, collapsed) in [("workbench.scm.action.collapseAllRepositories", true), ("workbench.scm.action.expandAllRepositories", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let handles: Vec<u64> = workbench.visible_scm_providers().iter().map(|provider| provider.handle).collect();
                for handle in handles {
                    match collapsed {
                        true => workbench.scm.collapsed_repositories.insert(handle),
                        false => workbench.scm.collapsed_repositories.remove(&handle),
                    };
                }
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("scm.openInTerminal", |workbench, args, _, _| workbench.open_scm_root_in_external_terminal(&args));
        self.commands.register("scm.openInIntegratedTerminal", |workbench, args, window, cx| {
            if let Some(root) = workbench.scm_root_of(&args) {
                workbench.create_terminal_in(root, window, cx);
            }
            Outcome::done()
        });
        self.register_scm_graph_actions();
    }

    fn register_scm_graph_actions(&mut self) {
        self.commands.register("workbench.scm.action.graph.refresh", |workbench, _, _, cx| {
            workbench.scm_graph.loaded_for = None;
            workbench.scm_graph.scroll.set_offset(gpui::point(px(0.), px(0.)));
            workbench.update_scm_graph(cx);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.scm.action.graph.revealCurrentHistoryItem", |workbench, _, _, cx| {
            workbench.reveal_current_scm_history_item();
            cx.notify();
            Outcome::done()
        });
        for (id, mode) in [("workbench.scm.action.graph.setListViewMode", "list"), ("workbench.scm.action.graph.setTreeViewMode", "tree")] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.workspace_storage.lock().unwrap().set(GRAPH_VIEW_MODE_KEY, mode.to_string());
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("workbench.scm.action.graph.viewChanges", |workbench, args, window, cx| workbench.view_scm_history_item_changes(&args, window, cx));
        self.commands.register("workbench.scm.action.graph.openFile", |workbench, args, window, cx| {
            let uri = args.get(1).map(|change| change["modifiedUri"].clone()).filter(Value::is_object);
            match uri {
                Some(uri) => workbench.execute_command("vscode.open", vec![uri], window, cx),
                None => Outcome::done(),
            }
        });
    }

    /// The `SCMInputHistory` navigator of a provider's input, loading `scm.history` on first use.
    fn scm_input_history(&mut self, handle: u64) -> Option<&mut HistoryNavigator> {
        let provider = self.scm.model.provider(handle)?;
        let (label, root) = (provider.label.clone(), provider.root_uri.clone());
        let histories = &mut self.scm.input_histories;
        if !histories.loaded {
            histories.loaded = true;
            let stored = self.workspace_storage.lock().unwrap().get(INPUT_HISTORY_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
            for entry in stored.as_ref().and_then(Value::as_array).into_iter().flatten() {
                let (Some(label), Some(values)) = (entry[0].as_str(), entry[2].as_array()) else { continue };
                let values: Vec<String> = values.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
                histories.persisted.push((label.to_string(), entry[1].clone(), HistoryNavigator::new(&values)));
            }
        }
        if root.is_null() {
            return Some(histories.in_memory.entry(handle).or_insert_with(|| HistoryNavigator::new(&[])));
        }
        let index = match histories.persisted.iter().position(|(known, uri, _)| *known == label && same_uri(uri, &root)) {
            Some(index) => index,
            None => {
                histories.persisted.push((label, root, HistoryNavigator::new(&[])));
                histories.persisted.len() - 1
            }
        };
        Some(&mut histories.persisted[index].2)
    }

    /// `SCMInputHistory.saveToStorage`: every history but an empty draft. Upstream saves on shutdown; the port saves
    /// on each change, as quick_open.rs `record_editor_history` does.
    fn save_scm_input_history(&self) {
        let raw: Vec<Value> = self.scm.input_histories.persisted.iter().filter(|(_, _, history)| !history.is_empty_draft()).map(|(label, uri, history)| json!([label, uri, history.values])).collect();
        self.workspace_storage.lock().unwrap().set(INPUT_HISTORY_KEY, Value::Array(raw).to_string());
    }

    /// `SCMInput.setValue(value, false)` of `$setInputBoxValue`, before the model takes the value: the draft becomes
    /// the last entry and the new value the draft.
    pub(super) fn record_scm_input_value(&mut self, method: &str, args: &[Value]) {
        if method != "$setInputBoxValue" {
            return;
        }
        let (Some(handle), Some(value)) = (args.first().and_then(Value::as_u64), args.get(1).and_then(Value::as_str)) else { return };
        let Some(current) = self.scm.model.provider(handle).map(|provider| provider.input.value.clone()) else { return };
        if current == value {
            return;
        }
        let Some(history) = self.scm_input_history(handle) else { return };
        history.replace_last(&current);
        history.add(value.to_string());
        self.save_scm_input_history();
    }

    /// `new SCMInput`: the input starts with its history's current value, which the host hears of
    /// (mainThreadSCM.ts `$registerSourceControl`).
    pub(super) fn restore_scm_input(&mut self, handle: u64, cx: &mut Context<Self>) {
        let Some(value) = self.scm_input_history(handle).map(|history| history.current().to_string()).filter(|value| !value.is_empty()) else { return };
        let Some(provider) = self.scm.model.provider_mut(handle) else { return };
        provider.input.value = value.clone();
        if self.scm.focused == Some(handle) {
            self.scm.input.set_value(value.clone());
        }
        self.host.send("ExtHostSCM", "$onInputBoxValueChange", vec![json!(handle), json!(value)]);
        self.sync_scm_input_model(handle, &value, cx);
    }

    /// `SCMInput.showPreviousHistoryValue` and `showNextHistoryValue` of the focused input; the caret goes to the
    /// start of a previous value and the end of a next one (scmInput.ts).
    fn show_scm_history_value(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.scm.focused else { return };
        let Some(current) = self.scm.model.provider(handle).map(|provider| provider.input.value.clone()) else { return };
        let Some(history) = self.scm_input_history(handle) else { return };
        let at_end = history.is_at_end();
        if at_end && !previous {
            return;
        }
        let mut changed = false;
        if at_end || !history.has(&current) {
            changed = history.replace_last(&current) != current;
            if !at_end {
                history.reset_cursor();
            }
        }
        let value = match previous {
            true => history.previous().to_string(),
            false => history.next().to_string(),
        };
        if changed {
            self.save_scm_input_history();
        }
        self.edit_scm_input(
            |input| {
                input.set_value(value);
                if previous {
                    input.select(0, 0);
                }
            },
            window,
            cx,
        );
    }

    /// scmViewPane.ts `focusInput`: the focused repository's input, or with it focused already the next or previous
    /// visible repository's.
    fn focus_scm_input_by(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.side_bar_view = SideBarView::Scm;
        cx.notify();
        let Some(focused) = self.scm.focused else { return };
        let repositories: Vec<u64> = self.visible_scm_providers().iter().map(|provider| provider.handle).collect();
        let has_focus = self.scm.input_focus.is_focused(window);
        if repositories.is_empty() || (repositories.len() == 1 && has_focus) {
            return;
        }
        let mut target = focused;
        if repositories.len() > 1 && has_focus {
            let index = repositories.iter().position(|handle| *handle == focused).map_or(-1, |index| index as isize);
            target = repositories[rot(index + delta, repositories.len())];
        }
        if !self.scm.model.provider(target).is_some_and(|provider| provider.input.visible) {
            return;
        }
        self.scm.collapsed_repositories.remove(&target);
        self.focus_scm_repository(Some(target));
        self.focus_scm_input(window, cx);
    }

    /// scmViewPane.ts `focusResourceGroup`: the first shown group of the focused repository, or with a group row
    /// focused the next or previous shown one.
    fn focus_scm_resource_group(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.side_bar_view = SideBarView::Scm;
        cx.notify();
        let Some(provider) = self.focused_scm_provider() else { return };
        let handle = provider.handle;
        let groups: Vec<(u64, bool)> = provider.groups.iter().map(|group| (group.handle, self.scm_group_shown(provider, group))).collect();
        let focused_group = match self.scm.selected {
            Some((provider, group, None)) if provider == handle && self.scm.list_focus.is_focused(window) => groups.iter().position(|(known, _)| *known == group),
            _ => None,
        };
        let next = match focused_group {
            None => groups.iter().find(|(_, shown)| *shown).map(|(group, _)| *group),
            Some(focused) => {
                let mut index = rot(focused as isize + delta, groups.len());
                while index != focused && !groups[index].1 {
                    index = rot(index as isize + delta, groups.len());
                }
                (index != focused).then_some(groups[index].0)
            }
        };
        let Some(group) = next else { return };
        self.scm.collapsed_repositories.remove(&handle);
        self.scm.selected = Some((handle, group, None));
        let focus = self.scm.list_focus.clone();
        window.focus(&focus, cx);
    }

    /// Whether the view shows a group's row (`tree.hasNode`), as `push_scm_repository_children` decides.
    fn scm_group_shown(&self, provider: &Provider, group: &crate::scm::Group) -> bool {
        let has_some_changes = provider.groups.iter().any(|group| !group.resources.is_empty());
        let button = provider.action_button.as_ref().filter(|button| button["command"].is_object() && self.setting("scm.showActionButton") != json!(false));
        let only_repository = self.visible_scm_providers().len() == 1 && button.is_none();
        (has_some_changes || only_repository) && !(group.hide_when_empty && group.resources.is_empty())
    }

    /// The root of the `ISCMProvider` argument (`MainThreadSCMProvider` marshalled as `{ $mid: 5, handle }`).
    fn scm_root_of(&self, args: &[Value]) -> Option<PathBuf> {
        let provider = self.scm.model.provider(args.first()?["handle"].as_u64()?)?;
        provider.root_uri["path"].as_str().map(PathBuf::from)
    }

    /// `scm.openInTerminal`, then externalTerminalService.ts `MacExternalTerminalService.spawnTerminal`:
    /// `open -a <terminal.external.osxExec> <root>`.
    fn open_scm_root_in_external_terminal(&self, args: &[Value]) -> Outcome {
        let Some(root) = self.scm_root_of(args) else { return Outcome::done() };
        let setting = self.setting("terminal.external.osxExec");
        let terminal = setting.as_str().filter(|app| !app.is_empty()).unwrap_or("Terminal.app");
        match std::process::Command::new("/usr/bin/open").arg("-a").arg(terminal).arg(&root).current_dir(&root).spawn() {
            Ok(_) => Outcome::done(),
            Err(error) => Outcome::error(format!("Opening {} in {terminal}: {error}", root.display())),
        }
    }

    /// The context keys of the view's title (`view`, `ContextKeys` of scmViewPane.ts) and its `config.` reads.
    pub(super) fn scm_view_context(&self, context: &mut HashMap<String, Value>) {
        let repositories: Vec<u64> = self.visible_scm_providers().iter().map(|provider| provider.handle).collect();
        let collapsible = repositories.len() > 1;
        let structural_diff_enabled = self.setting("scm.structuralDiff.enabled");
        context.insert("view".into(), json!(VIEW_PANE_ID));
        context.insert("scmRepositoryCount".into(), json!(repositories.len()));
        context.insert("scmRepositoryVisibleCount".into(), json!(repositories.len()));
        context.insert("scmViewMode".into(), json!(if self.scm_tree_mode() { "tree" } else { "list" }));
        context.insert("scmViewSortKey".into(), json!(self.scm_view_sort_key()));
        context.insert("scmViewShowStructuralDiff".into(), json!(self.scm_show_structural_diff_stored() && structural_diff_enabled == json!(true)));
        // The port has no `registerStructuralDiffProvider`, so no language has a structural diff provider.
        context.insert("scmViewHasStructuralDiffProvider".into(), json!(false));
        context.insert("config.scm.structuralDiff.enabled".into(), structural_diff_enabled);
        context.insert("scmViewIsAnyRepositoryCollapsible".into(), json!(collapsible));
        context.insert("scmViewAreAllRepositoriesCollapsed".into(), json!(collapsible && repositories.iter().all(|handle| self.scm.collapsed_repositories.contains(handle))));
    }

    /// scm_tree.rs `scm_sort_key`: `scm.viewSortKey` in workspace storage, `scm.defaultViewSortKey` when unset.
    fn scm_view_sort_key(&self) -> String {
        let stored = self.workspace_storage.lock().unwrap().get("scm.viewSortKey").map(str::to_string);
        stored.unwrap_or_else(|| self.setting("scm.defaultViewSortKey").as_str().unwrap_or("path").to_string())
    }

    fn scm_show_structural_diff_stored(&self) -> bool {
        self.workspace_storage.lock().unwrap().get(SHOW_STRUCTURAL_DIFF_KEY) == Some("true")
    }

    /// The view title's `...`: `MenuId.SCMTitle` but its navigation group, with the view's args.
    pub(super) fn scm_more_actions(&self, context: HashMap<String, Value>, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        self.more_actions_button("scm-title-more", MenuId::ScmTitle, context, args, foreground, cx)
    }

    /// A toolbar's "More Actions..." button: the menu's actions outside `navigation`, submenus inline between
    /// separators since the context menu draws no submenus.
    fn more_actions_button(&self, id: &'static str, menu: MenuId, context: HashMap<String, Value>, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        self.icon("ellipsis", 22., 16., foreground)
            .id(id)
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|icon| icon.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let actions = flatten_submenus(workbench.menu_actions_outside(menu, "navigation", &context));
                    workbench.show_context_menu_actions(actions, &context, Bounds::new(event.position, Default::default()), args.clone(), window, cx);
                }),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
    }

    /// `getMenuActions(...).filter(group => group[0] !== excluded)`.
    fn menu_actions_outside(&self, menu: MenuId, excluded: &str, context: &HashMap<String, Value>) -> Vec<MenuAction> {
        let registry: Vec<MenuItem> = self.menus.iter().filter(|item| item.menu != menu || item.group != excluded).cloned().collect();
        menu_actions(&registry, menu, context)
    }

    /// `MenuId.SCMSourceControl` on a repository row: its context with the `config.` and `remoteName` keys the
    /// terminal items read.
    pub(super) fn show_scm_source_control_menu(&mut self, provider: u64, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.scm.model.provider(provider) else { return };
        let mut context = self.scm_context(provider, None, None, window);
        let kind = self.setting("terminal.sourceControlRepositoriesKind");
        context.insert("config.terminal.sourceControlRepositoriesKind".into(), json!(kind.as_str().unwrap_or("integrated")));
        context.entry("remoteName".into()).or_insert(json!(""));
        let args = vec![Self::provider_arg(provider)];
        self.show_context_menu(MenuId::ScmSourceControl, &context, Bounds::new(position, Default::default()), args, window, cx);
    }

    /// `scm.graphView.viewMode` in workspace storage, `scm.defaultViewMode` when unset (`_getViewMode`).
    pub(super) fn scm_graph_view_mode(&self) -> &'static str {
        let stored = self.workspace_storage.lock().unwrap().get(GRAPH_VIEW_MODE_KEY).map(|mode| mode == "tree");
        match stored.unwrap_or(self.setting("scm.defaultViewMode") != json!("list")) {
            true => "tree",
            false => "list",
        }
    }

    /// The Graph view's context: `view`, the history item count, view mode and current ref keys.
    fn scm_graph_context(&self, provider: &Provider, window: &Window) -> HashMap<String, Value> {
        let mut context = self.scm_context(provider, None, None, window);
        let [current, remote, base] = &self.scm_graph.refs;
        context.insert("view".into(), json!(HISTORY_VIEW_PANE_ID));
        context.insert("scmHistoryItemCount".into(), json!(self.scm_graph.view_models.len()));
        context.insert("scmHistoryViewMode".into(), json!(self.scm_graph_view_mode()));
        context.insert("scmCurrentHistoryItemRefHasRemote".into(), json!(remote.is_some()));
        context.insert("scmCurrentHistoryItemRefHasBase".into(), json!(base.is_some()));
        // The 'auto' history items filter holds the current ref whenever there is one.
        context.insert("scmCurrentHistoryItemRefInFilter".into(), json!(current.is_some()));
        context
    }

    /// The Graph pane header's toolbar (`MenuId.SCMHistoryTitle`): the navigation actions, then "More Actions...".
    pub(super) fn scm_graph_title_actions(&self, provider: &Provider, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let context = self.scm_graph_context(provider, window);
        let args = vec![Self::provider_arg(provider)];
        let navigation: Vec<MenuItem> = self.menus.iter().filter(|item| item.menu == MenuId::ScmHistoryTitle && item.group == "navigation").cloned().collect();
        let mut bar = div().ml_auto().mr(px(4.)).flex().items_center();
        for action in menu_actions(&navigation, MenuId::ScmHistoryTitle, &context) {
            match action {
                MenuAction::Command { id, .. } if id == PICK_HISTORY_ITEM_REFS => bar = bar.child(self.scm_history_item_picker()),
                MenuAction::Command { id, title, enabled: true, .. } => bar = bar.child(self.command_button(&id, title, args.clone(), foreground, cx)),
                // actionbar.css: a disabled action's label at 0.4 opacity, without a click or hover background.
                MenuAction::Command { id, title, .. } => bar = bar.child(self.disabled_command_button(&id, title, foreground)),
                _ => {}
            }
        }
        bar.child(self.more_actions_button("scm-graph-title-more", MenuId::ScmHistoryTitle, context, args, foreground, cx))
    }

    fn disabled_command_button(&self, id: &str, title: String, foreground: Rgba) -> Div {
        let icon = match self.command_icons.get(id) {
            Some(CommandIcon::Codicon(name)) => self.icon(name, 22., 16., foreground),
            _ => div().px(px(4.)).child(title),
        };
        icon.opacity(0.4)
    }

    /// `SCMHistoryItemRefsActionViewItem` of the 'auto' filter: the `git-branch` codicon and "Auto" in
    /// `icon.foreground` (scm.css `.scm-graph-history-item-picker`), the current, remote and base refs' names as its
    /// tooltip.
    fn scm_history_item_picker(&self) -> gpui::Stateful<Div> {
        let foreground = self.color("icon.foreground");
        let tooltip = self.scm_graph.refs.iter().flatten().map(|reference| reference.name.as_str()).collect::<Vec<_>>().join(", ");
        div()
            .id("scm-graph-history-item-picker")
            .h(px(22.))
            .px(px(2.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .text_color(foreground)
            .font_weight(gpui::FontWeight::NORMAL)
            .text_size(px(11.))
            .line_height(px(16.))
            .cursor_pointer()
            .hover(|picker| picker.bg(self.color("toolbar.hoverBackground")))
            .child(self.icon("git-branch", 16., 16., foreground))
            .child(div().max_w(px(100.)).pr(px(2.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child("Auto"))
            .child(self.hover_target("scm-graph-history-item-picker-hover", HoverContent::Text(tooltip), HoverOptions::mouse()))
    }

    /// `SCMHistoryViewPane._onContextMenu` of a history item: `MenuId.SCMHistoryItemContext` but its inline group,
    /// run with the provider and the item; incoming and outgoing changes have no menu.
    pub(super) fn show_scm_history_item_menu(&mut self, handle: u64, index: usize, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view_model) = self.scm_graph.view_models.get(index) else { return };
        let Some(provider) = self.scm.model.provider(handle) else { return };
        if matches!(view_model.kind, HistoryItemKind::IncomingChanges | HistoryItemKind::OutgoingChanges) {
            return;
        }
        let history_item = &view_model.history_item;
        let mut context = self.scm_graph_context(provider, window);
        let current = self.scm_graph.refs[0].as_ref().map(|reference| reference.id.as_str());
        context.insert("scmHistoryItemHasCurrentHistoryItemRef".into(), json!(history_item.references.iter().any(|reference| Some(reference.id.as_str()) == current)));
        let args = vec![Self::provider_arg(provider), history_item_arg(history_item)];
        let id = history_item.id.clone();
        self.scm_graph.selected = Some(id);
        let actions = self.menu_actions_outside(MenuId::ScmHistoryItemContext, "inline", &context);
        self.show_context_menu_actions(actions, &context, Bounds::new(position, Default::default()), args, window, cx);
    }

    /// `_onContextMenu` of a history item change: `MenuId.SCMHistoryItemChangeContext` run with the item and the change.
    pub(super) fn show_scm_history_change_menu(&mut self, history_item: &HistoryItem, change: &Value, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.active_scm_provider() else { return };
        let context = self.scm_graph_context(provider, window);
        let args = vec![history_item_arg(history_item), change.clone()];
        let actions = self.menu_actions_outside(MenuId::ScmHistoryItemChangeContext, "inline", &context);
        self.show_context_menu_actions(actions, &context, Bounds::new(position, Default::default()), args, window, cx);
    }

    /// `SCMHistoryViewPane.revealCurrentHistoryItem`: selects the current ref's item and scrolls to it.
    fn reveal_current_scm_history_item(&mut self) {
        let graph = &mut self.scm_graph;
        let Some(revision) = graph.refs[0].as_ref().and_then(|current| current.revision.clone()) else { return };
        let Some(index) = graph.view_models.iter().position(|view_model| view_model.history_item.id == revision) else { return };
        let rows_before: usize = graph.view_models[..index].iter().map(|view_model| 1 + graph.changes.get(&view_model.history_item.id).and_then(Option::as_ref).map_or(0, Vec::len)).sum();
        graph.selected = Some(revision);
        graph.scroll.scroll_to_item(rows_before);
    }

    /// `workbench.scm.action.graph.viewChanges`: the multi diff editor of an item against its parent, or of the
    /// incoming or outgoing changes against the merge base.
    fn view_scm_history_item_changes(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(provider) = args.first().and_then(|provider| provider["handle"].as_u64()).and_then(|handle| self.scm.model.provider(handle)) else { return Outcome::done() };
        let id = args.get(1).and_then(|item| item["id"].as_str());
        let graph = &self.scm_graph;
        let Some(history_item) = graph.view_models.iter().map(|view_model| &view_model.history_item).find(|item| Some(item.id.as_str()) == id) else { return Outcome::done() };
        let [Some(current), remote, _] = &graph.refs else { return Outcome::done() };
        let merge_base = graph.merge_base.clone();
        let request = match (history_item.id.as_str(), remote) {
            (SCM_INCOMING_HISTORY_ITEM_ID, Some(remote)) => merge_base.map(|base| (format!("{} - {} \u{2194} {}", history_item.subject, current.name, remote.name), remote.id.clone(), base)),
            (SCM_OUTGOING_HISTORY_ITEM_ID, Some(remote)) => merge_base.map(|base| (format!("{} - {} \u{2194} {}", history_item.subject, remote.name, current.name), current.id.clone(), base)),
            _ => {
                let parent = match history_item.parent_ids.first() {
                    Some(parent) if parent == SCM_INCOMING_HISTORY_ITEM_ID && remote.is_some() => merge_base,
                    parent => parent.cloned(),
                };
                parent.map(|parent| (history_item_editor_title(history_item), history_item.id.clone(), parent))
            }
        };
        let Some((title, item_id, parent_id)) = request else { return Outcome::done() };
        let query = json!({ "repositoryId": format!("scm{}", provider.handle), "historyItemId": item_id, "historyItemParentId": parent_id, "historyItemDisplayId": "" });
        let source = json!({ "scheme": "scm-history-item", "path": provider.root_uri["path"], "query": query.to_string() });
        self.execute_command("_workbench.openMultiDiffEditor", vec![json!({ "title": title, "multiDiffSourceUri": source })], window, cx)
    }
}

/// util.ts `getHistoryItemEditorTitle`: the display id and the subject.
fn history_item_editor_title(history_item: &HistoryItem) -> String {
    format!("{} - {}", history_item.display_id.as_deref().unwrap_or(&history_item.id), history_item.subject)
}

/// The `ISCMHistoryItem` a history item action gets.
fn history_item_arg(history_item: &HistoryItem) -> Value {
    let references: Vec<Value> = history_item
        .references
        .iter()
        .map(|reference| json!({ "id": reference.id, "name": reference.name, "revision": reference.revision, "category": reference.category, "description": reference.description, "icon": reference.icon }))
        .collect();
    json!({
        "id": history_item.id,
        "parentIds": history_item.parent_ids,
        "subject": history_item.subject,
        "message": history_item.message,
        "displayId": history_item.display_id,
        "author": history_item.author,
        "authorEmail": history_item.author_email,
        "timestamp": history_item.timestamp,
        "references": references,
    })
}

/// Submenus as their actions between separators.
fn flatten_submenus(actions: Vec<MenuAction>) -> Vec<MenuAction> {
    let mut flat: Vec<MenuAction> = Vec::new();
    for action in actions {
        match action {
            MenuAction::Submenu { actions, .. } => {
                if flat.last().is_some_and(|last| *last != MenuAction::Separator) {
                    flat.push(MenuAction::Separator);
                }
                flat.extend(flatten_submenus(actions));
                flat.push(MenuAction::Separator);
            }
            MenuAction::Separator if flat.last().is_none_or(|last| *last == MenuAction::Separator) => {}
            action => flat.push(action),
        }
    }
    if flat.last() == Some(&MenuAction::Separator) {
        flat.pop();
    }
    flat
}

/// The menu items of scmViewPane.ts, scm.contribution.ts and scmHistoryViewPane.ts: menu, command or submenu, title,
/// group, order, `when`, precondition and `toggled`.
type Row = (MenuId, &'static str, &'static str, &'static str, f64, Option<&'static str>, Option<&'static str>, Option<&'static str>);
const SCM_MENUS: &[Row] = &[
    (MenuId::ScmTitle, "", "View & Sort", "0_view&sort", 1., Some("view == 'workbench.scm' && scmRepositoryCount != 0"), None, None),
    // Not listed: setListViewModeNavigation and setTreeViewModeNavigation, `isHiddenByDefault` and the port has no
    // toolbar context menu that shows a hidden action.
    (
        MenuId::ScmTitle,
        "workbench.scm.action.collapseAllRepositories",
        "Collapse All Repositories",
        "navigation",
        0.,
        Some("view == 'workbench.scm' && scmViewIsAnyRepositoryCollapsible == true && scmViewAreAllRepositoriesCollapsed == false"),
        None,
        None,
    ),
    (
        MenuId::ScmTitle,
        "workbench.scm.action.expandAllRepositories",
        "Expand All Repositories",
        "navigation",
        0.,
        Some("view == 'workbench.scm' && scmViewIsAnyRepositoryCollapsible == true && scmViewAreAllRepositoriesCollapsed == true"),
        None,
        None,
    ),
    (MenuId::ScmViewSort, "workbench.scm.action.setListViewMode", "View as List", "1_viewmode", 0., None, None, Some("scmViewMode == 'list'")),
    (MenuId::ScmViewSort, "workbench.scm.action.setTreeViewMode", "View as Tree", "1_viewmode", 0., None, None, Some("scmViewMode == 'tree'")),
    (MenuId::ScmViewSort, "workbench.scm.action.setSortKey.name", "Sort Changes by Name", "2_sort", 0., None, Some("scmViewMode == 'list'"), Some("scmViewSortKey == 'name'")),
    (MenuId::ScmViewSort, "workbench.scm.action.setSortKey.path", "Sort Changes by Path", "2_sort", 0., None, Some("scmViewMode == 'list'"), Some("scmViewSortKey == 'path'")),
    (MenuId::ScmViewSort, "workbench.scm.action.setSortKey.status", "Sort Changes by Status", "2_sort", 0., None, Some("scmViewMode == 'list'"), Some("scmViewSortKey == 'status'")),
    (
        MenuId::ScmViewSort,
        "workbench.scm.action.toggleShowStructuralDiff",
        "Show Structural Diff",
        "3_structuralDiff",
        1.,
        Some("config.scm.structuralDiff.enabled == true && scmViewHasStructuralDiffProvider"),
        None,
        Some("scmViewShowStructuralDiff"),
    ),
    (
        MenuId::ScmSourceControl,
        "scm.openInTerminal",
        "Open in External Terminal",
        "99_terminal",
        0.,
        Some("remoteName == '' && scmProviderHasRootUri == true && (config.terminal.sourceControlRepositoriesKind == 'external' || config.terminal.sourceControlRepositoriesKind == 'both')"),
        None,
        None,
    ),
    (
        MenuId::ScmSourceControl,
        "scm.openInIntegratedTerminal",
        "Open in Integrated Terminal",
        "99_terminal",
        0.,
        Some("scmProviderHasRootUri == true && (config.terminal.sourceControlRepositoriesKind == 'integrated' || config.terminal.sourceControlRepositoriesKind == 'both')"),
        None,
        None,
    ),
    (MenuId::ScmHistoryTitle, PICK_HISTORY_ITEM_REFS, "History Item Reference Picker", "navigation", 1., None, Some("scmHistoryItemCount != 0"), None),
    (MenuId::ScmHistoryTitle, "workbench.scm.action.graph.revealCurrentHistoryItem", "Go to Current History Item", "navigation", 2., None, Some("scmHistoryItemCount != 0 && scmCurrentHistoryItemRefInFilter == true"), None),
    (MenuId::ScmHistoryTitle, "workbench.scm.action.graph.refresh", "Refresh", "navigation", 1000., None, None, None),
    (MenuId::ScmHistoryTitle, "workbench.scm.action.graph.setListViewMode", "View as List", "9_viewmode", 1., None, None, Some("scmHistoryViewMode == 'list'")),
    (MenuId::ScmHistoryTitle, "workbench.scm.action.graph.setTreeViewMode", "View as Tree", "9_viewmode", 2., None, None, Some("scmHistoryViewMode == 'tree'")),
    (MenuId::ScmHistoryItemContext, "workbench.scm.action.graph.viewChanges", "Open Changes", "inline", 1., None, None, None),
    (MenuId::ScmHistoryItemContext, "workbench.scm.action.graph.viewChanges", "Open Changes", "0_view", 1., None, None, None),
    (MenuId::ScmHistoryItemChangeContext, "workbench.scm.action.graph.openFile", "Open File", "inline", 1., None, None, None),
    (MenuId::ScmHistoryItemChangeContext, "workbench.scm.action.graph.openFile", "Open File", "0_view", 1., None, None, None),
];

/// The codicons of the actions above (`icon` of their `Action2`).
const SCM_ICONS: [(&str, &str); 6] = [
    ("workbench.scm.action.collapseAllRepositories", "collapse-all"),
    ("workbench.scm.action.expandAllRepositories", "expand-all"),
    ("workbench.scm.action.graph.revealCurrentHistoryItem", "target"),
    ("workbench.scm.action.graph.refresh", "refresh"),
    ("workbench.scm.action.graph.viewChanges", "diff-multiple"),
    ("workbench.scm.action.graph.openFile", "go-to-file"),
];

/// scmHistoryViewPane.ts `PICK_HISTORY_ITEM_REFS_ACTION_ID`.
const PICK_HISTORY_ITEM_REFS: &str = "workbench.scm.action.graph.pickHistoryItemRefs";

pub fn menu_items() -> Vec<MenuItem> {
    let mut items = Vec::new();
    for (menu, id, title, group, order, when, precondition, toggled) in SCM_MENUS.iter().copied() {
        let entry = match id {
            "" => Entry::Submenu { submenu: MenuId::ScmViewSort, toggled: None },
            id => Entry::Command { id: id.to_string(), precondition: precondition.and_then(Expr::deserialize), toggled: Toggled::when(toggled.and_then(Expr::deserialize)), alt: None },
        };
        items.push(MenuItem { menu, entry, title: title.to_string(), group: group.to_string(), order, when: when.and_then(Expr::deserialize) });
    }
    items
}

pub fn command_icons() -> HashMap<String, CommandIcon> {
    SCM_ICONS.iter().map(|(id, icon)| (id.to_string(), CommandIcon::Codicon(icon.to_string()))).collect()
}
