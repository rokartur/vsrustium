//! The Timeline view of the Explorer (timelinePane.ts, timeline.contribution.ts) with mainThreadTimeline.ts: the
//! `MainThreadTimeline` requests, the section following the active editor or showing the workspace timeline, its
//! rows with their selection, keyboard navigation and menus, and the `timeline.*` commands.

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, Bounds, ClickEvent, Context, Div, FocusHandle, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, Pixels, Rgba, ScrollHandle, SharedString, Stateful, Window, canvas, div, prelude::*, px};
use regex::Regex;
use serde_json::{Map, Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::scm_view::{INDENT, ROW_HEIGHT, stop_action_mouse_down};
use super::search_view::SideBarView;
use crate::commands::Outcome;
use crate::contextkey::{Expr, JsRegex};
use crate::menus::{Entry, MenuAction, MenuId, MenuItem, Toggled, menu_actions};
use crate::timeline::{LOCAL_HISTORY_SOURCE, Row, Timeline, TimelineItem, TimelineProvider, TimelineRequest, TimelineResult, from_now_ago, uri_basename};

const EXCLUDE_SOURCES_KEY: &str = "timeline.excludeSources";
/// timeline.ts `TimelinePaneId`, the `view` context key of its menus.
const TIMELINE_VIEW_ID: &str = "timeline";
/// `LoadMoreCommand.handle`.
const LOAD_MORE_HANDLE: &str = "vscode-command:loadMore";
/// marshallingIds.ts `MarshalledId.TimelineActionContext`.
const TIMELINE_ACTION_CONTEXT_MID: u64 = 12;
/// timeline.ts `TimelineTriggerKind`.
const TRIGGER_INVOKE: u64 = 1;
pub(super) const TRIGGER_AUTOMATIC: u64 = 2;

pub struct TimelineView {
    pub(super) model: Timeline,
    /// `followActiveEditor` turned off ("Pin Current Timeline", "Pin Workspace Timeline", "Open Timeline").
    pinned: bool,
    /// The body was visible on the last render; becoming visible runs `onActiveEditorChanged` again.
    shown: bool,
    /// The active editor's resource the view last followed, and whether it showed the workspace timeline for none.
    followed: (Option<Value>, bool),
    /// The tree's selection, by item handle in the order it was made.
    selected: Vec<String>,
    /// The tree's focused row.
    focused: Option<String>,
    /// Where a Shift range selection starts.
    anchor: Option<String>,
    pub(super) focus: FocusHandle,
    scroll: ScrollHandle,
    /// The type navigation prefix and when its last key came.
    typed: Option<(String, Instant)>,
    /// The title toolbar's tab stop, its focused item and where each item painted.
    toolbar_focus: FocusHandle,
    toolbar_index: usize,
    toolbar_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
}

/// What a title toolbar item runs.
enum ToolbarRun {
    Command(String),
    Menu(Vec<MenuAction>, HashMap<String, Value>),
}

impl TimelineView {
    pub fn new(cx: &mut App) -> TimelineView {
        TimelineView {
            model: Timeline::default(),
            pinned: false,
            shown: false,
            followed: (None, false),
            selected: Vec::new(),
            focused: None,
            anchor: None,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            typed: None,
            toolbar_focus: cx.focus_handle(),
            toolbar_index: 0,
            toolbar_bounds: Rc::default(),
        }
    }
}

impl Workbench {
    /// timeline.contribution.ts and `TimelinePaneCommands`.
    pub(in crate::workbench) fn register_timeline_commands(&mut self) {
        self.commands.register("timeline.refresh", |workbench, _, _, cx| {
            workbench.load_timeline(true, None, TRIGGER_INVOKE, cx);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("timeline.toggleFollowActiveEditor", |workbench, _, _, cx| {
            workbench.timeline.pinned = !workbench.timeline.pinned;
            // Following again runs `onActiveEditorChanged` on the next render.
            workbench.timeline.shown = false;
            cx.notify();
            Outcome::done()
        });
        self.commands.register("timeline.showWorkspaceTimeline", |workbench, _, _, cx| {
            workbench.timeline.pinned = true;
            workbench.set_timeline_target(None, true, cx);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("timeline.compareSelected", |workbench, _, window, cx| workbench.compare_selected_timeline_items(window, cx));
        self.commands.register("files.openTimeline", |workbench, args, window, cx| {
            workbench.open_timeline(args.first(), window, cx);
            Outcome::done()
        });
    }

    /// One `MainThreadTimeline` request.
    pub(crate) fn handle_timeline_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let id = args.first().and_then(Value::as_str).unwrap_or_default().to_string();
        match method {
            "$registerTimelineProvider" | "$registerWorkspaceTimelineProvider" => {
                let Some(provider) = args.first().and_then(TimelineProvider::from_descriptor) else { return Outcome::done() };
                let id = provider.id.clone();
                if self.timeline.model.sources().iter().all(|source| source.id != id) {
                    self.register_exclude_source_command(&id);
                }
                match method {
                    "$registerTimelineProvider" => self.timeline.model.register(provider),
                    _ => self.timeline.model.register_workspace(provider),
                }
                self.load_timeline(true, Some(vec![id]), TRIGGER_AUTOMATIC, cx);
            }
            "$unregisterTimelineProvider" | "$unregisterWorkspaceTimelineProvider" => {
                let model = &mut self.timeline.model;
                let workspace = method == "$unregisterWorkspaceTimelineProvider";
                let providers = if workspace { &model.workspace_providers } else { &model.providers };
                if providers.iter().all(|provider| provider.id != id) {
                    return Outcome::done();
                }
                let remains = if workspace { model.unregister_workspace(&id) } else { model.unregister(&id) };
                match remains {
                    true => self.load_timeline(true, Some(vec![id]), TRIGGER_AUTOMATIC, cx),
                    false => self.commands.unregister(&format!("timeline.toggleExcludeSource:{id}")),
                }
            }
            "$emitTimelineChangeEvent" => self.timeline_changed(&id, false, args.get(1), cx),
            "$emitWorkspaceTimelineChangeEvent" => self.timeline_changed(&id, true, None, cx),
            _ => return Outcome::done(),
        }
        cx.notify();
        Outcome::done()
    }

    /// The pane's context keys and timelineService.ts `updateHasProviderContext`; `timelineFocused` while the tree
    /// has focus.
    pub(in crate::workbench) fn timeline_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        if self.timeline.focus.is_focused(window) {
            context.insert("timelineFocused".into(), json!(true));
        }
        let model = &self.timeline.model;
        context.insert("timelineHasProvider".into(), json!(!model.sources().is_empty()));
        context.insert("timelineHasWorkspaceProvider".into(), json!(!model.workspace_providers.is_empty()));
        context.insert("timelineFollowActiveEditor".into(), json!(!self.timeline.pinned));
        context.insert("timelineForWorkspace".into(), json!(model.workspace));
        context.insert("timelineExcludeSources".into(), json!(self.stored_excluded_sources()));
    }

    /// `isBodyVisible`: expanded, in the Explorer on screen.
    fn timeline_visible(&self) -> bool {
        self.view_pane_expanded(super::view_panes::PaneContainer::Explorer, TIMELINE_VIEW_ID) && self.side_bar_view == SideBarView::Explorer && self.side_bar_visible()
    }

    /// `onActiveEditorChanged` and `setExpanded`, checked on every render: the body follows the active editor while
    /// visible, and a pinned body reloads once it shows again.
    pub(in crate::workbench) fn update_timeline(&mut self, cx: &mut Context<Self>) {
        if !self.timeline_visible() {
            self.timeline.shown = false;
            return;
        }
        let became_visible = !std::mem::replace(&mut self.timeline.shown, true);
        if self.timeline.pinned {
            if became_visible {
                self.load_timeline(true, None, TRIGGER_AUTOMATIC, cx);
            }
            return;
        }
        // An editor of multiple resources, such as a commit opened from the timeline, keeps the current timeline.
        if self.editors.active_multi_diff().is_some() {
            return;
        }
        let target = self.timeline_target();
        let workspace = target.is_none() && !self.timeline.model.workspace_providers.is_empty();
        let followed = (target, workspace);
        if became_visible || self.timeline.followed != followed {
            self.timeline.followed = followed.clone();
            self.follow_timeline_target(followed.0, followed.1, cx);
        }
    }

    /// The active editor's resource, without the schemes that have no timeline.
    fn timeline_target(&self) -> Option<Value> {
        let editor = self.editors.active()?;
        let excluded = ["vscode-settings", "webview-panel", "walkThrough"];
        (!excluded.contains(&editor.document().scheme.as_str())).then(|| crate::editor::uri_value(&editor.document().scheme, &editor.document().path))
    }

    /// The rest of `onActiveEditorChanged`: the same target only loads missing or stale sources (`reloadTimeline`);
    /// without a resource the workspace timeline shows when a provider has one.
    fn follow_timeline_target(&mut self, target: Option<Value>, workspace: bool, cx: &mut Context<Self>) {
        let model = &self.timeline.model;
        let same = match (&target, &model.uri) {
            (Some(target), Some(shown)) => same_resource(target, shown),
            (None, _) => workspace && model.workspace,
            (Some(_), None) => false,
        };
        if !same {
            return self.set_timeline_target(target, workspace, cx);
        }
        let excluded = self.timeline_excluded_sources();
        let page_size = self.timeline_page_size();
        let sources: Vec<String> = model.sources().iter().map(|source| source.id.clone()).filter(|id| !excluded.contains(id)).collect();
        for source in sources {
            let request = match self.timeline.model.timeline(&source) {
                Some(timeline) if !timeline.stale => continue,
                Some(_) => self.timeline.model.reload(&source, page_size, &excluded),
                None => self.timeline.model.request(&source, true, None, page_size, &excluded),
            };
            if let Some(request) = request {
                self.send_timeline_request(request, TRIGGER_AUTOMATIC, cx);
            }
        }
    }

    /// timelinePane.ts `setUriCore`: shows the file or folder, the workspace timeline when `uri` is `None` and
    /// `workspace` is set, or nothing.
    fn set_timeline_target(&mut self, uri: Option<Value>, workspace: bool, cx: &mut Context<Self>) {
        let view = &mut self.timeline;
        view.model.workspace = uri.is_none() && workspace;
        view.model.uri = uri;
        view.selected.clear();
        view.focused = None;
        view.anchor = None;
        self.load_timeline(true, None, TRIGGER_AUTOMATIC, cx);
    }

    /// timeline.contribution.ts `OpenTimelineAction`: the argument's resource, else the focused Explorer row's, else
    /// the active editor's, pinned in the opened view (timelineService.ts `setUri`).
    fn open_timeline(&mut self, arg: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) {
        let explorer_row = self.folder.as_ref().and_then(|tree| tree.focused.clone()).filter(|_| self.explorer.focus.is_focused(window));
        let uri = match arg.filter(|arg| arg["scheme"].is_string()) {
            Some(uri) => Some(uri.clone()),
            None => explorer_row.map(|path| crate::editor::uri_value("file", &path)).or_else(|| self.timeline_target()),
        };
        self.side_bar_view = SideBarView::Explorer;
        self.expand_pane(super::view_panes::PaneContainer::Explorer, TIMELINE_VIEW_ID, cx);
        self.timeline.shown = self.timeline_visible();
        if let Some(uri) = uri {
            self.timeline.pinned = true;
            self.set_timeline_target(Some(uri), false, cx);
        }
        window.focus(&self.timeline.focus.clone(), cx);
        cx.notify();
    }

    /// timelinePane.ts `loadTimeline`: every source, or `sources`, asked for the shown target.
    pub(super) fn load_timeline(&mut self, reset: bool, sources: Option<Vec<String>>, trigger: u64, cx: &mut Context<Self>) {
        let page_size = self.timeline_page_size();
        if sources.is_none() && reset {
            self.timeline.model.clear(true, page_size);
        }
        if !self.timeline.model.has_target() {
            return self.timeline.model.clear(false, page_size);
        }
        if !self.timeline_visible() {
            return;
        }
        let excluded = self.timeline_excluded_sources();
        let sources = sources.unwrap_or_else(|| self.timeline.model.sources().iter().map(|source| source.id.clone()).collect());
        for source in sources {
            if let Some(request) = self.timeline.model.request(&source, reset, None, page_size, &excluded) {
                self.send_timeline_request(request, trigger, cx);
            }
        }
    }

    /// mainThreadTimeline.ts `provideTimeline` and `provideWorkspaceTimeline`: `ExtHostTimeline.$getTimeline(id, uri,
    /// options, token)` with no uri for the workspace timeline, its answer handled as `handleRequest` does.
    fn send_timeline_request(&mut self, request: TimelineRequest, trigger: u64, cx: &mut Context<Self>) {
        if request.source == LOCAL_HISTORY_SOURCE {
            let answer = request.uri.as_ref().map(|uri| self.local_history_timeline(uri));
            self.timeline.model.respond(&request, answer);
            return cx.notify();
        }
        let mut options = Map::new();
        if let Some(cursor) = &request.cursor {
            options.insert("cursor".into(), json!(cursor));
        }
        if let Some(limit) = request.limit {
            options.insert("limit".into(), json!(limit));
        }
        options.insert("triggerKind".into(), json!(trigger));
        options.insert("cacheResults".into(), json!(true));
        options.insert("resetCache".into(), json!(request.reset));
        let uri = request.uri.clone().unwrap_or(Value::Null);
        let answer = self.host.call("ExtHostTimeline", "$getTimeline", vec![json!(request.source), uri, Value::Object(options), Value::Null]);
        cx.spawn(async move |workbench, cx| {
            let answer = answer.await.ok().and_then(Result::ok);
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.timeline.model.respond(&request, answer.as_ref().and_then(TimelineResult::from_value));
                cx.notify();
            });
        })
        .detach();
    }

    /// `onTimelineChanged`: a change of the shown resource (or of every resource), or of the shown workspace
    /// timeline, reloads that source, or marks it stale while the body is hidden.
    pub(super) fn timeline_changed(&mut self, source: &str, workspace: bool, uris: Option<&Value>, cx: &mut Context<Self>) {
        let model = &self.timeline.model;
        let affected = match (&model.uri, workspace) {
            (_, true) => model.workspace,
            (Some(shown), false) => uris.and_then(Value::as_array).is_none_or(|uris| uris.iter().any(|uri| same_resource(uri, shown))),
            (None, false) => false,
        };
        if !affected || model.timeline(source).is_none() {
            return;
        }
        if !self.timeline_visible() {
            if let Some(timeline) = self.timeline.model.timelines.iter_mut().find(|timeline| timeline.source == source) {
                timeline.stale = true;
            }
            return;
        }
        let excluded = self.timeline_excluded_sources();
        if let Some(request) = self.timeline.model.reload(source, self.timeline_page_size(), &excluded) {
            self.send_timeline_request(request, TRIGGER_AUTOMATIC, cx);
        }
    }

    /// timelinePane.ts `loadMore`: one page more of the merged rows.
    fn load_more_timeline(&mut self, cx: &mut Context<Self>) {
        if !self.timeline.model.pending.is_empty() {
            return;
        }
        let excluded = self.timeline_excluded_sources();
        let visible = self.timeline.model.rows(&excluded, 0).iter().filter(|row| matches!(row, Row::Item { .. })).count();
        self.timeline.model.max_item_count = visible + self.timeline_page_size();
        self.load_timeline(false, None, TRIGGER_AUTOMATIC, cx);
        cx.notify();
    }

    /// `pageSize`: `timeline.pageSize`, 50 by default.
    // shortcut: `null` takes the default too, not the view's height in rows; upgrade with the configuration defaults.
    fn timeline_page_size(&self) -> usize {
        self.setting("timeline.pageSize").as_u64().filter(|size| *size > 0).map_or(50, |size| size as usize)
    }

    /// `timeline.excludeSources` in the profile storage, `[]` when unset.
    fn stored_excluded_sources(&self) -> String {
        self.profile_storage.lock().unwrap().get(EXCLUDE_SOURCES_KEY).map_or_else(|| "[]".to_string(), str::to_owned)
    }

    /// The sources "Filter Timeline" unchecked.
    fn timeline_excluded_sources(&self) -> HashSet<String> {
        serde_json::from_str::<Vec<String>>(&self.stored_excluded_sources()).unwrap_or_default().into_iter().collect()
    }

    /// `updateTimelineSourceFilters`: `timeline.toggleExcludeSource:<id>`, then `onStorageServiceChanged`.
    pub(super) fn register_exclude_source_command(&mut self, source: &str) {
        let source = source.to_string();
        self.commands.register(format!("timeline.toggleExcludeSource:{source}"), move |workbench, _, _, cx| {
            let mut excluded = workbench.timeline_excluded_sources();
            if !excluded.remove(&source) {
                excluded.insert(source.clone());
            }
            let mut stored: Vec<&String> = excluded.iter().collect();
            stored.sort();
            workbench.profile_storage.lock().unwrap().set(EXCLUDE_SOURCES_KEY, json!(stored).to_string());
            let model = &workbench.timeline.model;
            let missing: Vec<String> = model.sources().iter().map(|source| source.id.clone()).filter(|id| !excluded.contains(id) && model.timeline(id).is_none()).collect();
            if !missing.is_empty() {
                workbench.load_timeline(true, Some(missing), TRIGGER_AUTOMATIC, cx);
            }
            cx.notify();
            Outcome::done()
        });
    }

    /// `TimelinePane.compareSelected`: the older of two selected items' resources against the newer one.
    fn compare_selected_timeline_items(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let model = &self.timeline.model;
        let mut selection: Vec<&TimelineItem> = self.timeline.selected.iter().filter_map(|handle| model.item(handle)).collect();
        if selection.len() != 2 {
            return Outcome::done();
        }
        selection.sort_by_key(|item| item.timestamp);
        let [older, newer] = [selection[0], selection[1]];
        let (Some(older_uri), Some(newer_uri)) = (older.resource_uri.clone(), newer.resource_uri.clone()) else { return Outcome::done() };
        let now = now_ms();
        // Items without a label are named after their resource, so they are described by their time instead.
        let describe = |item: &TimelineItem| if item.label_from_resource || item.label.is_empty() { from_now_ago(item.timestamp, now) } else { item.label.clone() };
        let label = format!("{} ({}) \u{2194} {} ({})", uri_basename(&older_uri), describe(older), uri_basename(&newer_uri), describe(newer));
        self.execute_command("_workbench.diff", vec![older_uri, newer_uri, json!(label)], window, cx)
    }

    /// The pane's menus: `TimelinePaneCommands`' title and item actions, the "Filter Timeline" submenu of every
    /// source, and the `timeline/title` and `timeline/item/context` contributions.
    fn timeline_menus(&self) -> Vec<MenuItem> {
        let mut items: Vec<MenuItem> = self.menus.iter().filter(|item| matches!(item.menu, MenuId::TimelineTitle | MenuId::TimelineItemContext)).cloned().collect();
        let command = |id: &str, toggled: Option<Expr>| Entry::Command { id: id.to_string(), precondition: None, toggled: Toggled::when(toggled), alt: None };
        let mut add = |menu, entry, title: &str, group: &str, order, when: Option<&str>| {
            items.push(MenuItem { menu, entry, title: title.to_string(), group: group.to_string(), order, when: when.and_then(Expr::deserialize) });
        };
        add(MenuId::TimelineTitle, command("timeline.refresh", None), "Refresh", "navigation", 99., None);
        add(MenuId::TimelineTitle, command("timeline.toggleFollowActiveEditor", None), "Pin Current Timeline", "navigation", 98., Some("timelineFollowActiveEditor"));
        add(MenuId::TimelineTitle, command("timeline.toggleFollowActiveEditor", None), "Unpin Current Timeline", "navigation", 98., Some("!timelineFollowActiveEditor"));
        add(MenuId::TimelineTitle, Entry::Submenu { submenu: MenuId::TimelineFilterSubMenu, toggled: None }, "Filter Timeline", "navigation", 100., None);
        add(MenuId::TimelineTitle, command("timeline.showWorkspaceTimeline", None), "Pin Workspace Timeline", "1_workspace", 0., Some("timelineHasWorkspaceProvider && !timelineForWorkspace"));
        add(MenuId::TimelineItemContext, command("timeline.compareSelected", None), "Compare Selected", "3_compare", 0., Some("timelineCanCompareSelection"));
        // localHistoryCommands.ts: the entry actions and the title's Find Entry to Restore.
        let single = "timelineItem == 'localHistory:item' && !listMultiSelection";
        let compare_with_selected = format!("{single} && localHistoryItemSelectedForCompare");
        let reveal = format!("{single} && resourceScheme == 'file'");
        for (id, title, group, order, when) in [
            ("compareWithFile", "Compare with File", "1_compare", 1., single),
            ("compareWithPrevious", "Compare with Previous", "1_compare", 2., single),
            ("compareWithSelected", "Compare with Selected", "2_compare_with", 1., &compare_with_selected),
            ("selectForCompare", "Select for Compare", "2_compare_with", 2., single),
            ("open", "Show Contents", "3_contents", 1., single),
            ("restore", "Restore Contents", "3_contents", 2., single),
            ("revealInOS", "Reveal in Finder", "4_reveal", 1., &reveal),
            ("rename", "Rename", "5_edit", 1., single),
            ("delete", "Delete", "5_edit", 2., "timelineItem == 'localHistory:item'"),
        ] {
            add(MenuId::TimelineItemContext, command(&format!("workbench.action.localHistory.{id}"), None), title, group, order, Some(when));
        }
        add(MenuId::TimelineTitle, command("workbench.action.localHistory.restoreViaPicker", None), "Local History: Find Entry to Restore...", "submenu", 1., Some("config.workbench.localHistory.enabled"));
        for source in self.timeline.model.sources() {
            let excluded = JsRegex::new(&format!(r"\b{}\b", regex::escape(&source.id)), "").map(|regex| Expr::regex("timelineExcludeSources", regex).negate());
            add(MenuId::TimelineFilterSubMenu, command(&format!("timeline.toggleExcludeSource:{}", source.id), excluded), &source.label, "navigation", 0., None);
        }
        items
    }

    /// The menus' context: the keybinding keys, `view`, and the `config.` keys their `when` clauses read, which
    /// upstream's context key service resolves from the configuration.
    fn timeline_menu_context(&self, menus: &[MenuItem], window: &Window) -> HashMap<String, Value> {
        static CONFIG_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"config\.[\w.-]+").expect("valid pattern"));
        let mut context = self.key_context(window);
        context.insert("view".into(), json!(TIMELINE_VIEW_ID));
        context.insert("localHistoryItemSelectedForCompare".into(), json!(self.local_history_selected_for_compare.is_some()));
        for when in menus.iter().filter_map(|item| item.when.as_ref()) {
            for key in CONFIG_KEY.find_iter(&when.serialize()) {
                let key = key.as_str();
                context.insert(key.to_string(), self.setting(&key["config.".len()..]));
            }
        }
        context
    }

    /// `getItemContext`: the item's `timelineItem`, and the list selection keys set per item since they do not
    /// apply to an item outside the selection.
    fn timeline_item_context(&self, item: &TimelineItem, mut context: HashMap<String, Value>) -> HashMap<String, Value> {
        let selection = self.timeline_multi_selection(item);
        if let Some(context_value) = &item.context_value {
            context.insert("timelineItem".into(), json!(context_value));
        }
        context.insert("listMultiSelection".into(), json!(selection.is_some()));
        context.insert("listDoubleSelection".into(), json!(selection.as_ref().is_some_and(|selection| selection.len() == 2)));
        let can_compare = selection.as_ref().is_some_and(|selection| selection.len() == 2 && selection.iter().all(|item| item.resource_uri.is_some()));
        context.insert("timelineCanCompareSelection".into(), json!(can_compare));
        context
    }

    /// `getMultiSelection`: the selected items when `item` is one of several selected.
    fn timeline_multi_selection(&self, item: &TimelineItem) -> Option<Vec<&TimelineItem>> {
        let model = &self.timeline.model;
        let items: Vec<&TimelineItem> = self.timeline.selected.iter().filter_map(|handle| model.item(handle)).collect();
        (items.len() > 1 && items.iter().any(|selected| selected.source == item.source && selected.handle == item.handle)).then_some(items)
    }

    /// `TimelineActionRunner.runAction`: `(item, uri, source, selectedItems?)`, the selection of the item's provider
    /// only, each item as a `TimelineActionContext` the extension host resolves.
    fn timeline_item_args(&self, item: &TimelineItem) -> Vec<Value> {
        let uri = self.timeline.model.uri.clone().unwrap_or(Value::Null);
        let action_context = |item: &TimelineItem| json!({ "$mid": TIMELINE_ACTION_CONTEXT_MID, "handle": item.handle, "source": item.source, "uri": uri });
        let mut args = vec![action_context(item), uri.clone(), json!(item.source)];
        if let Some(selection) = self.timeline_multi_selection(item) {
            args.push(selection.into_iter().filter(|selected| selected.source == item.source).map(action_context).collect());
        }
        args
    }

    /// `TimelinePane.onContextMenu`: focuses the row and shows `MenuId.TimelineItemContext` but its inline group.
    fn show_timeline_item_menu(&mut self, handle: &str, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.timeline.focus.clone(), cx);
        self.timeline.focused = Some(handle.to_string());
        let Some(item) = self.timeline.model.item(handle) else { return };
        let menus: Vec<MenuItem> = self.timeline_menus().into_iter().filter(|menu| !(menu.menu == MenuId::TimelineItemContext && menu.group == "inline")).collect();
        let context = self.timeline_item_context(item, self.timeline_menu_context(&menus, window));
        let actions = menu_actions(&menus, MenuId::TimelineItemContext, &context);
        let args = self.timeline_item_args(item);
        self.show_context_menu_actions(actions, &context, anchor, args, window, cx);
    }

    /// The tree's `onDidOpen`: an item runs its command, "Load more" loads the next page.
    /// `tree.onDidOpen`: `_workbench.open` and `_workbench.diff` get the open event's editor options after their
    /// arguments (listService.ts: a click preserves focus, Enter and a double click pin and focus the editor).
    fn open_timeline_row(&mut self, handle: &str, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if handle == LOAD_MORE_HANDLE {
            return self.load_more_timeline(cx);
        }
        let Some(command) = self.timeline.model.item(handle).and_then(|item| item.command.clone()) else { return };
        let Some(id) = command["id"].as_str() else { return };
        let mut args = command["arguments"].as_array().cloned().unwrap_or_default();
        if matches!(id, "_workbench.open" | "_workbench.diff") {
            args.push(json!({ "editorOptions": { "preserveFocus": preserve_focus, "pinned": !preserve_focus } }));
        }
        drop(self.execute_command(id, args, window, cx));
    }

    /// The handles of the rows in order, "Load more" last.
    fn timeline_row_handles(&self) -> Vec<String> {
        let excluded = self.timeline_excluded_sources();
        let rows = self.timeline.model.rows(&excluded, 0);
        rows.iter()
            .map(|row| match row {
                Row::Item { item, .. } => item.handle.clone(),
                Row::LoadMore { .. } => LOAD_MORE_HANDLE.to_string(),
            })
            .collect()
    }

    /// A click on a row (listWidget.ts `MouseController`): Cmd toggles the row in the selection, Shift selects the
    /// range from the anchor, a plain click selects and opens it.
    fn click_timeline_row(&mut self, handle: &str, modifiers: Modifiers, click_count: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.timeline.focus.clone(), cx);
        let view = &mut self.timeline;
        view.focused = Some(handle.to_string());
        cx.notify();
        if modifiers.platform {
            match view.selected.iter().position(|selected| selected == handle) {
                Some(index) => drop(view.selected.remove(index)),
                None => view.selected.push(handle.to_string()),
            }
            view.anchor = Some(handle.to_string());
            return;
        }
        if modifiers.shift {
            return self.select_timeline_range(handle);
        }
        let view = &mut self.timeline;
        view.selected = vec![handle.to_string()];
        view.anchor = Some(handle.to_string());
        self.open_timeline_row(handle, click_count < 2, window, cx);
    }

    fn select_timeline_range(&mut self, handle: &str) {
        let handles = self.timeline_row_handles();
        let view = &mut self.timeline;
        let anchor = view.anchor.as_deref().and_then(|anchor| handles.iter().position(|row| row == anchor)).unwrap_or(0);
        let Some(target) = handles.iter().position(|row| row == handle) else { return };
        view.selected = handles[anchor.min(target)..=anchor.max(target)].to_vec();
    }

    /// The tree's keys (listWidget.ts `KeyboardController`, listCommands.ts and the type navigation): arrows, Home,
    /// End and the page keys move the focus and the selection (Shift extends it), Enter opens, Escape clears the
    /// selection, Cmd+A selects every row, a letter focuses the next row whose label starts with what was typed.
    fn timeline_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let handles = self.timeline_row_handles();
        if handles.is_empty() {
            return;
        }
        let current = self.timeline.focused.as_ref().and_then(|focused| handles.iter().position(|handle| handle == focused));
        let page = (f32::from(self.timeline.scroll.bounds().size.height) / ROW_HEIGHT).floor().max(1.) as usize;
        let last = handles.len() - 1;
        let target = match keystroke.key.as_str() {
            "down" => current.map_or(0, |current| (current + 1).min(last)),
            "up" => current.map_or(0, |current| current.saturating_sub(1)),
            "pagedown" => current.map_or(0, |current| (current + page).min(last)),
            "pageup" => current.map_or(0, |current| current.saturating_sub(page)),
            "home" => 0,
            "end" => last,
            // A browser fires `contextmenu` at the focused row on Shift+F10; the menu anchors at the row.
            "tab" if modifiers.shift => {
                cx.stop_propagation();
                return self.focus_timeline_toolbar(window, cx);
            }
            "f10" if modifiers.shift => {
                let Some(current) = current else { return };
                cx.stop_propagation();
                let anchor = self.timeline.scroll.bounds_for_item(current).unwrap_or(self.timeline.scroll.bounds());
                return self.show_timeline_item_menu(&handles[current].clone(), anchor, window, cx);
            }
            "enter" => {
                cx.stop_propagation();
                if let Some(current) = current {
                    let handle = handles[current].clone();
                    self.timeline.selected = vec![handle.clone()];
                    self.open_timeline_row(&handle, false, window, cx);
                }
                return cx.notify();
            }
            "escape" if !self.timeline.selected.is_empty() => {
                cx.stop_propagation();
                self.timeline.selected.clear();
                return cx.notify();
            }
            "a" if modifiers.platform => {
                cx.stop_propagation();
                self.timeline.selected = handles;
                return cx.notify();
            }
            key if key.chars().count() == 1 && !modifiers.platform && !modifiers.control && !modifiers.alt => {
                let Some(target) = self.type_navigation_target(key, &handles, current) else { return };
                cx.stop_propagation();
                self.timeline.focused = Some(handles[target].clone());
                self.timeline.scroll.scroll_to_item(target);
                return cx.notify();
            }
            _ => return,
        };
        cx.stop_propagation();
        let handle = handles[target].clone();
        self.timeline.focused = Some(handle.clone());
        if modifiers.shift {
            self.select_timeline_range(&handle);
        } else {
            // timelinePane.ts creates its tree without `selectionNavigation`: arrows move the focus, not the selection.
            self.timeline.anchor = Some(handle);
        }
        self.timeline.scroll.scroll_to_item(target);
        cx.notify();
    }

    /// typeNavigation `TypeNavigationController.onInput`: keys typed within a second build a prefix, matched
    /// against the labels (`TimelineKeyboardNavigationLabelProvider`) from the focused row on, wrapping around.
    fn type_navigation_target(&mut self, key: &str, handles: &[String], current: Option<usize>) -> Option<usize> {
        let now = Instant::now();
        let prefix = match self.timeline.typed.take() {
            Some((typed, at)) if now.duration_since(at) < Duration::from_secs(1) => typed + &key.to_lowercase(),
            _ => key.to_lowercase(),
        };
        // A new prefix starts after the focused row, a longer one may still match it.
        let start = current.map_or(0, |current| if prefix.chars().count() == 1 { current + 1 } else { current });
        self.timeline.typed = Some((prefix.clone(), now));
        let model = &self.timeline.model;
        (0..handles.len()).map(|offset| (start + offset) % handles.len()).find(|&index| model.item(&handles[index]).is_some_and(|item| item.label.to_lowercase().starts_with(&prefix)))
    }

    /// The Timeline pane of the Explorer's pane view: the shown resource as its description, the title menu, and
    /// the message or the rows (timelinePane.css).
    pub(in crate::workbench) fn timeline_pane(&self, window: &Window, cx: &mut Context<Self>) -> super::view_panes::ViewPane {
        let model = &self.timeline.model;
        // timelinePane.ts `updateFilename`.
        let label = model.target_label();
        let description = match self.timeline.pinned {
            true if !label.is_empty() => format!("{label} (pinned)"),
            _ => label,
        };
        let excluded = self.timeline_excluded_sources();
        let local_history = self.setting("workbench.localHistory.enabled") == json!(true);
        let message = model.message(&excluded, local_history, self.scm.model.providers.len());
        let menus = self.timeline_menus();
        let has_inline = menus.iter().any(|item| item.menu == MenuId::TimelineItemContext && item.group == "inline");
        let row_context = has_inline.then(|| self.timeline_menu_context(&menus, window));
        let tree_focused = self.timeline.focus.is_focused(window);
        let rows: Vec<Stateful<Div>> = model
            .rows(&excluded, now_ms())
            .into_iter()
            .map(|row| match row {
                Row::Item { item, relative_time, duplicate } => {
                    let inline = row_context.as_ref().map(|context| self.timeline_inline_actions(&menus, item, context.clone(), cx));
                    self.timeline_item_row(item, relative_time, duplicate, tree_focused, inline, cx)
                }
                Row::LoadMore { loading } => self.timeline_load_more_row(loading, tree_focused, cx),
            })
            .collect();
        let message = message.map(|message| div().absolute().top_0().left_0().right_0().pt(px(10.)).px(px(22.)).opacity(0.5).child(message));
        let tree = div()
            .id("timeline-tree")
            .track_focus(&self.timeline.focus)
            .on_key_down(cx.listener(Self::timeline_key_down))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.timeline.scroll)
            .children(rows);
        let body = div().relative().size_full().flex().flex_col().children(message).child(tree).children(self.high_contrast_list_outline(tree_focused, self.timeline.focused.is_some()));
        super::view_panes::ViewPane {
            id: TIMELINE_VIEW_ID,
            title: "Timeline".into(),
            description: Some(description).filter(|description| !description.is_empty()),
            actions: Some(self.timeline_title_actions(window, cx).into_any_element()),
            show_actions: self.timeline.focus.is_focused(window),
            body: body.into_any_element(),
        }
    }

    /// The pane's title toolbar (`MenuId.TimelineTitle`): the navigation actions as icons, the "Filter Timeline"
    /// submenu as a dropdown, the rest under "More Actions...". Its commands run with the shown uri.
    /// The toolbar's items in order: their codicon (None for the command's own icon), title and what they run.
    fn timeline_toolbar(&self, window: &Window) -> Vec<(Option<&'static str>, String, ToolbarRun)> {
        let menus = self.timeline_menus();
        let context = self.timeline_menu_context(&menus, window);
        let navigation: Vec<MenuItem> = menus.iter().filter(|item| item.menu != MenuId::TimelineTitle || item.group == "navigation").cloned().collect();
        let secondary: Vec<MenuItem> = menus.iter().filter(|item| item.menu != MenuId::TimelineTitle || item.group != "navigation").cloned().collect();
        let mut items = Vec::new();
        for action in menu_actions(&navigation, MenuId::TimelineTitle, &context) {
            match action {
                MenuAction::Command { id, title, .. } => {
                    let icon = match id.as_str() {
                        "timeline.refresh" => Some("refresh"),
                        "timeline.toggleFollowActiveEditor" => Some(if self.timeline.pinned { "pinned" } else { "pin" }),
                        _ => None,
                    };
                    items.push((icon, title, ToolbarRun::Command(id)));
                }
                MenuAction::Submenu { title, actions, .. } => items.push((Some("filter"), title, ToolbarRun::Menu(actions, context.clone()))),
                MenuAction::Separator => {}
            }
        }
        let secondary_actions = menu_actions(&secondary, MenuId::TimelineTitle, &context);
        if !secondary_actions.is_empty() {
            items.push((Some("ellipsis"), "More Actions...".into(), ToolbarRun::Menu(secondary_actions, context)));
        }
        items
    }

    /// The pane's title toolbar (`MenuId.TimelineTitle`), one tab stop (actionbar.ts): the focused item draws the focus
    /// outline, Left and Right move it.
    fn timeline_title_actions(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let args = vec![self.timeline.model.uri.clone().unwrap_or(Value::Null)];
        let toolbar_focused = self.timeline.toolbar_focus.is_focused(window);
        // paneview.css: 20px action labels 4px apart; the port's icons are 22px.
        let mut bar = div().id("timeline-toolbar").track_focus(&self.timeline.toolbar_focus).on_key_down(cx.listener(Self::timeline_toolbar_key_down)).flex().gap(px(2.));
        for (index, (icon, title, run)) in self.timeline_toolbar(window).into_iter().enumerate() {
            let item = match (icon, run) {
                (None, ToolbarRun::Command(id)) => self.command_button(&id, title, args.clone(), foreground, cx),
                (Some(icon), ToolbarRun::Command(id)) => self.timeline_action(icon, id, title, args.clone(), foreground, cx),
                (icon, ToolbarRun::Menu(actions, context)) => self.timeline_dropdown(icon.unwrap_or("ellipsis"), title, actions, context, args.clone(), foreground, cx),
            };
            let bounds = self.timeline.toolbar_bounds.clone();
            let tracker = canvas(move |painted, _, _| bounds.borrow_mut().insert(index, painted), |_, _, _, _| {}).absolute().size_full();
            let focused = toolbar_focused && index == self.timeline.toolbar_index;
            bar = bar.child(item.relative().child(tracker).when(focused, |item| item.child(div().absolute().inset_0().rounded(px(5.)).border_1().border_color(self.color("focusBorder")))));
        }
        bar
    }

    /// `.pane:focus-within` of the tree or the toolbar.
    pub(super) fn timeline_focus_within(&self, window: &Window) -> bool {
        self.timeline.focus.is_focused(window) || self.timeline.toolbar_focus.is_focused(window)
    }

    /// Shift+Tab from the tree: the toolbar's tab stop, its first item focused with its hover shown.
    pub(super) fn focus_timeline_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.timeline.toolbar_index = 0;
        window.focus(&self.timeline.toolbar_focus.clone(), cx);
        self.show_timeline_toolbar_hover(window, cx);
    }

    fn show_timeline_toolbar_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.timeline.toolbar_index;
        let Some((icon, title, _)) = self.timeline_toolbar(window).into_iter().nth(index) else { return };
        let Some(bounds) = self.timeline.toolbar_bounds.borrow().get(&index).copied() else { return };
        let id = SharedString::from(format!("timeline-action-hover-{}", icon.unwrap_or("command")));
        self.show_managed_hover(id, HoverContent::Text(title), HoverOptions::managed(), bounds, window, cx);
    }

    fn timeline_toolbar_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.timeline_toolbar(window);
        if items.is_empty() {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let index = self.timeline.toolbar_index.min(items.len() - 1);
        match event.keystroke.key.as_str() {
            "right" => self.timeline.toolbar_index = (index + 1) % items.len(),
            "left" => self.timeline.toolbar_index = (index + items.len() - 1) % items.len(),
            "tab" if modifiers.shift => {
                cx.stop_propagation();
                self.dismiss_hover(cx);
                return self.focus_pane_header(super::view_panes::TIMELINE_VIEW, window, cx);
            }
            "tab" => {
                cx.stop_propagation();
                self.dismiss_hover(cx);
                return window.focus(&self.timeline.focus.clone(), cx);
            }
            "enter" | "space" => {
                cx.stop_propagation();
                self.dismiss_hover(cx);
                let args = vec![self.timeline.model.uri.clone().unwrap_or(Value::Null)];
                let (_, _, run) = items.into_iter().nth(index).expect("index is below the item count");
                match run {
                    ToolbarRun::Command(id) => drop(self.execute_command(&id, args, window, cx)),
                    ToolbarRun::Menu(actions, context) => {
                        let bounds = self.timeline.toolbar_bounds.borrow().get(&index).copied().unwrap_or_default();
                        let anchor = Bounds::new(gpui::point(bounds.origin.x, bounds.origin.y + bounds.size.height), Default::default());
                        self.show_context_menu_actions(actions, &context, anchor, args, window, cx);
                    }
                }
                return;
            }
            _ => return,
        }
        cx.stop_propagation();
        self.show_timeline_toolbar_hover(window, cx);
        cx.notify();
    }

    fn timeline_action(&self, icon: &'static str, command: String, title: String, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        self.timeline_icon_button(icon, title, foreground).on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            drop(workbench.execute_command(&command, args.clone(), window, cx));
        }))
    }

    /// A toolbar icon that shows `actions` as a menu below it (`DropdownMenuActionViewItem`).
    #[allow(clippy::too_many_arguments, reason = "the menu, its context and arguments, and the icon")]
    fn timeline_dropdown(&self, icon: &'static str, title: String, actions: Vec<MenuAction>, context: HashMap<String, Value>, args: Vec<Value>, foreground: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        self.timeline_icon_button(icon, title, foreground).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_context_menu_actions(actions.clone(), &context, Bounds::new(event.position, Default::default()), args.clone(), window, cx);
            }),
        )
    }

    fn timeline_icon_button(&self, icon: &'static str, title: String, foreground: Rgba) -> Stateful<Div> {
        self.icon(icon, 22., 16., foreground)
            .id(SharedString::from(format!("timeline-action-{icon}")))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|action| action.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, stop_action_mouse_down)
            .child(self.hover_target(SharedString::from(format!("timeline-action-hover-{icon}")), HoverContent::Text(title), HoverOptions::action_item()))
    }

    /// `getItemActions`: the `inline` group of `MenuId.TimelineItemContext` for the item, shown on hover.
    fn timeline_inline_actions(&self, menus: &[MenuItem], item: &TimelineItem, context: HashMap<String, Value>, cx: &mut Context<Self>) -> Div {
        let context = self.timeline_item_context(item, context);
        let inline: Vec<MenuItem> = menus.iter().filter(|menu| menu.menu == MenuId::TimelineItemContext && menu.group == "inline").cloned().collect();
        let args = self.timeline_item_args(item);
        let foreground = self.foreground_or("sideBar.foreground");
        let mut actions = div().flex().flex_none().invisible().group_hover(SharedString::from(format!("timeline-item-{}", item.handle)), |actions| actions.visible());
        for action in menu_actions(&inline, MenuId::TimelineItemContext, &context) {
            if let MenuAction::Command { id, title, .. } = action {
                actions = actions.child(self.command_button(&id, title, args.clone(), foreground, cx));
            }
        }
        actions
    }

    /// `TimelineTreeRenderer.renderElement`: icon, label and description, the relative time and the inline
    /// actions; a time equal to the one above shows as a faint line until the row is hovered or selected.
    fn timeline_item_row(&self, item: &TimelineItem, relative_time: String, duplicate: bool, tree_focused: bool, inline: Option<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let id = SharedString::from(format!("timeline-item-{}", item.handle));
        let selected = self.timeline.selected.contains(&item.handle);
        let duplicate = duplicate && !selected;
        let icon = item.theme_icon.as_ref().map(|(name, color)| self.icon(name, 16., 16., color.as_deref().and_then(|color| self.theme.color(color)).unwrap_or(foreground)));
        let (click_handle, menu_handle) = (item.handle.clone(), item.handle.clone());
        let timestamp = div()
            .flex_none()
            .ml(px(2.))
            .mr(px(4.))
            .opacity(0.5)
            .overflow_hidden()
            .whitespace_nowrap()
            .child(div().when(duplicate, |time| time.invisible().group_hover(id.clone(), |time| time.visible())).child(relative_time))
            .when(duplicate, |container| container.w(px(10.)).group_hover(id.clone(), |container| container.w_auto()));
        self.timeline_row(id.clone(), &item.handle, selected, tree_focused)
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.click_timeline_row(&click_handle, event.modifiers(), event.click_count(), window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_timeline_item_menu(&menu_handle, Bounds::new(event.position, Default::default()), window, cx)))
            .child(div().w(px(22.)).flex_none().children(icon))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .flex()
                    .items_center()
                    .child(item.label.clone())
                    .children(item.description.clone().map(|description| div().ml(px(6.)).text_size(px(12.)).opacity(0.7).child(description))),
            )
            .child(timestamp)
            .children(inline)
            .when(duplicate, |row| row.child(div().absolute().top_0().bottom_0().right(px(10.)).w(px(1.)).bg(foreground).opacity(0.125).group_hover(id.clone(), |line| line.invisible())))
            .children(item.tooltip.clone().map(|tooltip| self.hover_target(SharedString::from(format!("timeline-hover-{}", item.handle)), HoverContent::Text(tooltip), HoverOptions::instant())))
    }

    /// `LoadMoreCommand`: "Load more", "Loading..." while a request is out; with `timeline.pageOnScroll` it loads as
    /// soon as it is painted in view (`onDidScrollToEnd`).
    fn timeline_load_more_row(&self, loading: bool, tree_focused: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let page_on_scroll = self.setting("timeline.pageOnScroll") != json!(false);
        let entity = cx.entity().downgrade();
        let scrolled_to_end = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                if window.content_mask().bounds.intersects(&bounds) {
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |workbench, cx| workbench.load_more_timeline(cx));
                    });
                }
            },
        )
        .absolute()
        .size_full();
        let selected = self.timeline.selected.iter().any(|handle| handle == LOAD_MORE_HANDLE);
        self.timeline_row("timeline-load-more".into(), LOAD_MORE_HANDLE, selected, tree_focused)
            .on_click(cx.listener(|workbench, event: &ClickEvent, window, cx| workbench.click_timeline_row(LOAD_MORE_HANDLE, event.modifiers(), event.click_count(), window, cx)))
            .child(div().w(px(22.)).flex_none())
            .child(if loading { "Loading..." } else { "Load more" })
            .when(page_on_scroll && !loading, |row| row.child(scrolled_to_end))
    }

    /// views.css `.custom-view-tree-node-item`: a 22px row, a leaf of a tree without twisties; the focused tree
    /// draws the active selection colors and outlines its focused row (listWidget.ts).
    fn timeline_row(&self, id: SharedString, handle: &str, selected: bool, tree_focused: bool) -> Stateful<Div> {
        let focused = tree_focused && self.timeline.focused.as_deref() == Some(handle);
        let (selection, selection_foreground) = match tree_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| focused);
        div()
            .id(id.clone())
            .group(id)
            .relative()
            .h(px(ROW_HEIGHT))
            .flex_none()
            // list.css rows: 4px from the pane's edges with rounded corners, as the Explorer's; the tree has `hide-arrows`
            // (views.css: no twistie width), the indent measured against the reference with ui-pair.
            .mx(px(4.))
            .rounded(px(4.))
            .pl(px(INDENT))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            // list.css: a selected row keeps its selection background under the pointer.
            .when(!selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .when(selected, |row| row.bg(selection).when_some(selection_foreground, |row, foreground| row.text_color(foreground)))
            .when_some(focus_outline, |row, outline| row.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(outline)))
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |now| now.as_millis() as i64)
}

/// `extUri.isEqual`, or for `file` and `git` resources the same `fsPath`.
fn same_resource(a: &Value, b: &Value) -> bool {
    let part = |uri: &Value, key: &str| uri[key].as_str().unwrap_or_default().to_string();
    let same = |key: &str| part(a, key) == part(b, key);
    let file_like = |uri: &Value| matches!(uri["scheme"].as_str(), Some("file" | "git"));
    (same("scheme") && same("authority") && same("path") && same("query")) || (same("path") && file_like(a) && file_like(b))
}
