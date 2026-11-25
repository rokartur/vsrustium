//! Extension tree views (mainThreadTreeViews.ts, treeView.ts `TreeView`, viewsExtensionPoint.ts): the views
//! extensions contribute to the Explorer, their items fetched from `ExtHostTreeViews`, drawn as panes.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, StatefulInteractiveElement, Styled, Task, Window, div, img, prelude::FluentBuilder, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::commands::Outcome;
use crate::contextkey::Expr;
use crate::custom_tree::{Collapsible, CustomTree, TreeItem};
use crate::menus::MenuId;

#[cfg(test)]
mod tests;

/// A view of `contributes.views.explorer`.
pub(super) struct ContributedView {
    pub id: &'static str,
    pub name: String,
    when: Option<Expr>,
}

#[derive(Default)]
pub(super) struct TreeViews {
    pub views: Vec<ContributedView>,
    trees: HashMap<String, CustomTree>,
    /// Views whose tree shows, as last told to the host (`$setVisible`).
    visible: HashSet<String>,
    focus: HashMap<String, FocusHandle>,
    /// The roots being fetched per view, which a reveal waits for (AsyncDataTree shares a node's refresh).
    root_fetches: HashMap<String, Shared<Task<()>>>,
}

/// Pane ids are `&'static str`; a contributed view's id is interned once, so a reinstall does not leak again.
fn static_id(id: &str) -> &'static str {
    static IDS: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Mutex::default);
    let mut ids = IDS.lock().unwrap();
    if let Some(&interned) = ids.get(id) {
        return interned;
    }
    let interned: &'static str = Box::leak(id.to_owned().into_boxed_str());
    ids.insert(interned);
    interned
}

/// viewsExtensionPoint.ts: the tree views of the Explorer container (`contributes.views.explorer`), in extension
/// order. Views of other containers are not drawn yet: the port has no view container registry for them.
pub(super) fn contributed_views(extensions: &[exthost::scan::ScannedExtension]) -> Vec<ContributedView> {
    let mut views = Vec::new();
    for extension in extensions {
        for view in extension.description["contributes"]["views"]["explorer"].as_array().into_iter().flatten() {
            // A webview view (`type: webview`) needs `MainThreadWebviewViews`, not ported.
            let (Some(id), Some(name)) = (view["id"].as_str(), view["name"].as_str()) else { continue };
            if view["type"] == "webview" {
                continue;
            }
            views.push(ContributedView { id: static_id(id), name: name.to_owned(), when: view["when"].as_str().and_then(Expr::deserialize) });
        }
    }
    views
}

impl Workbench {
    pub(crate) fn set_tree_views(&mut self, extensions: &[exthost::scan::ScannedExtension]) {
        self.tree_views.views = contributed_views(extensions);
    }

    /// The contributed views whose `when` holds, which the Explorer shows as panes.
    pub(super) fn shown_tree_views(&self) -> Vec<&'static str> {
        let views = self.tree_views.views.iter().filter(|view| view.when.as_ref().is_none_or(|when| when.evaluate(&self.context_keys)));
        views.map(|view| view.id).collect()
    }

    pub(crate) fn handle_tree_view_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let null = Value::Null;
        let arg = |index: usize| args.get(index).unwrap_or(&null);
        let id = arg(0).as_str().unwrap_or_default().to_owned();
        match method {
            "$registerTreeViewDataProvider" => {
                let tree = self.tree_views.trees.entry(id.clone()).or_default();
                tree.has_provider = true;
                tree.show_collapse_all = arg(1)["showCollapseAll"] == true;
                tree.can_select_many = arg(1)["canSelectMany"] == true;
                let visible = self.tree_views.visible.contains(&id);
                self.host.send("ExtHostTreeViews", "$setVisible", vec![json!(id), json!(visible)]);
                cx.notify();
            }
            "$refresh" => {
                let Some(tree) = self.tree_views.trees.get_mut(&id).filter(|tree| tree.has_provider) else { return Outcome::done() };
                let parents = match arg(1).as_object().filter(|items| !items.is_empty()) {
                    Some(items) => tree.refresh(items),
                    None => vec![None],
                };
                // treeView.ts `refresh`: the tree is created when its view is visible; before that nothing is fetched.
                if self.tree_views.visible.contains(&id) {
                    for parent in parents {
                        if let Some(tree) = self.tree_views.trees.get_mut(&id) {
                            match &parent {
                                None => tree.roots = None,
                                Some(parent) => drop(tree.children.remove(parent)),
                            }
                        }
                    }
                    self.fetch_missing_children(cx);
                }
                cx.notify();
            }
            "$reveal" => return self.reveal_tree_item(id, arg(1).clone(), arg(2).clone(), window, cx),
            "$setMessage" => self.update_tree(&id, |tree| tree.message = Some(arg(1).as_str().or(arg(1)["value"].as_str()).unwrap_or_default().to_owned()).filter(|message| !message.is_empty()), cx),
            "$setTitle" => self.update_tree(&id, |tree| (tree.title, tree.description) = (arg(1).as_str().map(str::to_owned), arg(2).as_str().map(str::to_owned)), cx),
            "$setBadge" => self.update_tree(&id, |tree| tree.badge = Some(arg(1).clone()).filter(|badge| !badge.is_null()), cx),
            "$disposeTree" => self.update_tree(&id, |tree| *tree = CustomTree::default(), cx),
            "$resolveDropFileData" => return Outcome::error("Unknown tree"),
            // `$logResolveTreeNodeFailure` is telemetry.
            _ => {}
        }
        Outcome::done()
    }

    fn update_tree(&mut self, id: &str, update: impl FnOnce(&mut CustomTree), cx: &mut Context<Self>) {
        update(self.tree_views.trees.entry(id.to_owned()).or_default());
        cx.notify();
    }

    /// mainThreadTreeViews.ts `$reveal`: opens the view, then (its tree shown) fetches the roots if there are none,
    /// expands the parents, selects and focuses the item and expands it `expand` levels (3 at most).
    fn reveal_tree_item(&mut self, id: String, item_info: Value, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.shown_tree_views().contains(&id.as_str()) {
            drop(self.execute_command("workbench.view.explorer", Vec::new(), window, cx));
            self.expand_explorer_pane(&id, cx);
            self.sync_tree_views(cx);
        }
        let (sender, receiver) = oneshot::channel();
        let handle = item_info["item"]["handle"].as_str().map(str::to_owned);
        let parents: Vec<String> = item_info["parentChain"].as_array().into_iter().flatten().filter_map(|parent| parent["handle"].as_str().map(str::to_owned)).collect();
        let select = options["select"] == true;
        let focus = options["focus"] == true;
        let levels = match &options["expand"] {
            Value::Number(levels) => levels.as_u64().unwrap_or(0).min(3),
            Value::Bool(true) => 1,
            _ => 0,
        };
        cx.spawn_in(window, async move |workbench, cx| {
            let shown = workbench.update(cx, |workbench, _| workbench.tree_views.visible.contains(&id) && workbench.tree_views.trees.get(&id).is_some_and(|tree| tree.has_provider));
            if let (Ok(true), Some(handle)) = (shown, handle) {
                let roots = workbench.update(cx, |workbench, cx| workbench.tree_views.trees[&id].roots.is_none().then(|| workbench.fetch_roots(&id, cx)));
                if let Ok(Some(roots)) = roots {
                    roots.await;
                }
                let mut to_expand: Vec<String> = parents;
                let mut level = 0;
                loop {
                    let fetches = workbench.update(cx, |workbench, cx| workbench.expand_tree_items(&id, &to_expand, cx));
                    let Ok(fetches) = fetches else { break };
                    for fetch in fetches {
                        fetch.await;
                    }
                    let next = workbench.update(cx, |workbench, _| {
                        let tree = &workbench.tree_views.trees[&id];
                        if level == 0 {
                            return vec![handle.clone()].into_iter().filter(|handle| tree.items.contains_key(handle)).collect();
                        }
                        to_expand.iter().flat_map(|parent| tree.children.get(parent).cloned().unwrap_or_default()).collect::<Vec<String>>()
                    });
                    let Ok(next) = next else { break };
                    if level == 0 {
                        let _ = workbench.update_in(cx, |workbench, window, cx| workbench.select_tree_item(&id, &handle, select, focus, window, cx));
                    }
                    level += 1;
                    if level > levels || next.is_empty() {
                        break;
                    }
                    to_expand = next;
                }
            }
            let _ = sender.send(Ok(Value::Null));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// Expands the items and fetches the children nobody fetched yet, returning those fetches.
    fn expand_tree_items(&mut self, id: &str, handles: &[String], cx: &mut Context<Self>) -> Vec<Task<()>> {
        let Some(tree) = self.tree_views.trees.get_mut(id) else { return Vec::new() };
        let mut missing = Vec::new();
        for handle in handles {
            let Some(item) = tree.items.get(handle).filter(|item| item.collapsible != Collapsible::None) else { continue };
            if tree.expanded.insert(item.handle.clone()) {
                self.host.send("ExtHostTreeViews", "$setExpanded", vec![json!(id), json!(handle), json!(true)]);
            }
            if !tree.children.contains_key(handle) {
                missing.push(handle.clone());
            }
        }
        cx.notify();
        if missing.is_empty() {
            return Vec::new();
        }
        vec![self.fetch_children(id.to_owned(), Some(missing), cx)]
    }

    fn select_tree_item(&mut self, id: &str, handle: &str, select: bool, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let tree = self.tree_views.trees.get_mut(id).expect("revealed in a known tree");
        if select {
            tree.selection = vec![handle.to_owned()];
        }
        tree.focus = Some(handle.to_owned());
        self.host.send("ExtHostTreeViews", "$setSelectionAndFocus", vec![json!(id), json!(tree.selection), json!(handle)]);
        if focus && let Some(focus) = self.tree_views.focus.get(id) {
            window.focus(&focus.clone(), cx);
        }
        cx.notify();
    }

    /// `TreeViewDataProvider.getChildrenBatch`: `$getChildren` answers one `[index, ...items]` group per parent.
    fn fetch_children(&mut self, id: String, parents: Option<Vec<String>>, cx: &mut Context<Self>) -> Task<()> {
        let tree = self.tree_views.trees.entry(id.clone()).or_default();
        let keys: Vec<Option<String>> = match &parents {
            None => vec![None],
            Some(parents) => parents.iter().cloned().map(Some).collect(),
        };
        tree.fetching.extend(keys.iter().cloned());
        let reply = self.host.call("ExtHostTreeViews", "$getChildren", vec![json!(id), json!(parents)]);
        cx.spawn(async move |workbench, cx| {
            let groups = match reply.await {
                Ok(Ok(Value::Array(groups))) => groups,
                Ok(Err(error)) => {
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.notify_error(error["message"].as_str().unwrap_or_default());
                        cx.notify();
                    });
                    Vec::new()
                }
                _ => Vec::new(),
            };
            let _ = workbench.update(cx, |workbench, cx| {
                if parents.is_none() {
                    workbench.tree_views.root_fetches.remove(&id);
                }
                let Some(tree) = workbench.tree_views.trees.get_mut(&id) else { return };
                for key in &keys {
                    tree.fetching.remove(key);
                }
                let mut children: Vec<Vec<Value>> = vec![Vec::new(); keys.len()];
                for group in groups {
                    let Value::Array(mut group) = group else { continue };
                    let Some(index) = group.first().and_then(Value::as_u64).map(|index| index as usize).filter(|index| *index < keys.len()) else { continue };
                    group.remove(0);
                    children[index] = group;
                }
                for (key, items) in keys.iter().zip(children) {
                    tree.set_children(key.as_deref(), items);
                }
                cx.notify();
            });
        })
    }

    /// The view's root fetch, started unless one is under way.
    fn fetch_roots(&mut self, id: &str, cx: &mut Context<Self>) -> Shared<Task<()>> {
        if let Some(fetch) = self.tree_views.root_fetches.get(id) {
            return fetch.clone();
        }
        let fetch = self.fetch_children(id.to_owned(), None, cx).shared();
        self.tree_views.root_fetches.insert(id.to_owned(), fetch.clone());
        fetch
    }

    /// Fetches the roots of each shown tree with a provider and the children of its expanded items.
    fn fetch_missing_children(&mut self, cx: &mut Context<Self>) {
        let visible: Vec<String> = self.tree_views.visible.iter().cloned().collect();
        for id in visible {
            let Some(tree) = self.tree_views.trees.get(&id).filter(|tree| tree.has_provider) else { continue };
            if tree.roots.is_none() {
                drop(self.fetch_roots(&id, cx));
                continue;
            }
            let missing: Vec<String> = tree.missing_children().into_iter().filter(|handle| !tree.fetching.contains(&Some(handle.clone()))).collect();
            if !missing.is_empty() {
                self.fetch_children(id, Some(missing), cx).detach();
            }
        }
    }

    /// treeView.ts `onDidChangeBodyVisibility`: `$setVisible` when a view's tree shows or hides; the first show
    /// activates `onView:<id>` (`TreeView.activate`) and fetches the roots.
    pub(super) fn sync_tree_views(&mut self, cx: &mut Context<Self>) {
        for id in self.shown_tree_views() {
            self.tree_views.focus.entry(id.to_owned()).or_insert_with(|| cx.focus_handle());
        }
        let explorer = self.side_bar_view == super::search_view::SideBarView::Explorer && self.is_part_visible(super::layout::Part::SideBar);
        for id in self.shown_tree_views().into_iter().chain(self.tree_views.visible.iter().map(|id| static_id(id)).collect::<Vec<_>>()) {
            let visible = explorer && self.shown_tree_views().contains(&id) && self.explorer_pane_expanded(id);
            if visible == self.tree_views.visible.contains(id) {
                continue;
            }
            if visible {
                self.tree_views.visible.insert(id.to_owned());
                drop(self.host.activate_by_event(&format!("onView:{id}")));
            } else {
                self.tree_views.visible.remove(id);
            }
            if self.tree_views.trees.get(id).is_some_and(|tree| tree.has_provider) {
                self.host.send("ExtHostTreeViews", "$setVisible", vec![json!(id), json!(visible)]);
            }
        }
        self.fetch_missing_children(cx);
    }

    /// The pane of a contributed view: its title (the view's `name`, or `TreeView.title`), title actions, message
    /// and rows.
    pub(super) fn tree_view_pane(&self, id: &'static str, window: &Window, cx: &mut Context<Self>) -> super::view_panes::ExplorerPane {
        let focus = self.tree_views.focus[id].clone();
        let name = self.tree_views.views.iter().find(|view| view.id == id).map(|view| view.name.clone()).unwrap_or_default();
        let empty = CustomTree::default();
        let tree = self.tree_views.trees.get(id).unwrap_or(&empty);
        let title = tree.title.clone().unwrap_or(name);
        let foreground = self.foreground_or("sideBar.foreground");
        let mut context = self.key_context(window);
        context.insert("view".into(), json!(id));
        let actions = self.inline_actions(MenuId::ViewTitle, &context, Vec::new(), foreground, cx).into_any_element();
        let message = tree.message.clone().map(|message| div().pt(px(4.)).pb(px(4.)).pl(px(18.)).pr(px(12.)).child(message));
        let focused = focus.is_focused(window);
        let rows: Vec<AnyElement> = tree.rows().into_iter().enumerate().map(|(index, (item, depth))| self.tree_row(id, tree, item, depth, index, focused, window, cx).into_any_element()).collect();
        let body = div()
            .id(id)
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .children(message)
            .children(rows)
            .into_any_element();
        super::view_panes::ExplorerPane { id, title, description: tree.description.clone(), actions: Some(actions), body }
    }

    #[allow(clippy::too_many_arguments)]
    fn tree_row(&self, id: &'static str, tree: &CustomTree, item: &TreeItem, depth: usize, index: usize, focused: bool, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let handle = item.handle.clone();
        let selected = tree.selection.contains(&handle);
        let is_focus = focused && tree.focus.as_ref() == Some(&handle);
        let expanded = tree.expanded.contains(&handle);
        let mut context = self.key_context(window);
        context.insert("view".into(), json!(id));
        context.insert("viewItem".into(), item.dto["contextValue"].clone());
        // treeView.ts `TreeViewItemHandleArg`, revived by the host into the extension's element.
        let args = vec![json!({ "$treeViewId": id, "$treeItemHandle": handle })];
        let actions = self.inline_actions(MenuId::ViewItemContext, &context, args.clone(), foreground, cx);
        let row_id = gpui::SharedString::from(format!("tree-{id}-{handle}"));
        let twistie = match (item.collapsible, expanded) {
            (Collapsible::None, _) => div().size(px(16.)).flex_none(),
            (_, true) => self.icon("chevron-down", 16., 16., foreground),
            (_, false) => self.icon("chevron-right", 16., 16., foreground),
        };
        let toggled = handle.clone();
        let twistie = twistie.id(("tree-twistie", index)).on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            workbench.toggle_tree_item(id, &toggled, cx);
        }));
        let (selection, selection_foreground) = match focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let tooltip = item.dto["tooltip"].as_str().or(item.dto["tooltip"]["value"].as_str()).map(str::to_owned);
        let label = item.label();
        let clicked = handle.clone();
        let command = item.dto["command"].clone();
        let context_menu = context.clone();
        let menu_args = args.clone();
        div()
            .id(row_id.clone())
            .group(row_id.clone())
            .relative()
            .h(px(22.))
            .mx(px(4.))
            .pl(px(11. + 8. * depth as f32))
            .pr(px(12.))
            .flex()
            .items_center()
            .rounded(px(4.))
            // list.css: .monaco-list.mouse-support .monaco-list-row is cursor: pointer.
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .when(selected, |row| row.bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)))
            .when_some(self.theme.color("list.focusOutline").filter(|_| is_focus), |row, color| row.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color)))
            .when_some(tooltip, |row, tooltip| row.child(self.hover_target(("tree-hover", index), HoverContent::Text(tooltip), HoverOptions::mouse())))
            .child(twistie)
            .children(self.tree_item_icon(item, foreground).map(|icon| icon.ml(px(3.)).mr(px(6.))))
            .child(div().flex_none().whitespace_nowrap().when(item.strikethrough(), |label| label.line_through()).child(label))
            .children(item.description().map(|description| div().flex_1().min_w_0().ml(px(6.5)).overflow_hidden().whitespace_nowrap().text_size(px(11.7)).opacity(0.7).child(description)))
            .child(div().ml_auto().invisible().group_hover(row_id, |actions| actions.visible()).when(selected || is_focus, |actions| actions.visible()).child(actions))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.tree_item_clicked(id, &clicked, &command, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                workbench.show_context_menu(MenuId::ViewItemContext, &context_menu, gpui::Bounds::new(event.position, Default::default()), menu_args.clone(), window, cx);
            }))
    }

    /// treeView.ts `TreeRenderer`: a theme icon as a codicon (`file` and `folder` take the resource's file icon), an
    /// icon path as an image, else the resource's file icon.
    fn tree_item_icon(&self, item: &TreeItem, foreground: gpui::Rgba) -> Option<Div> {
        let resource = item.dto["resourceUri"]["path"].as_str().map(std::path::PathBuf::from);
        match item.theme_icon() {
            Some("file" | "folder") | None if item.dto["icon"].is_null() => {
                let path = resource?;
                let language = self.languages.language_of(&path, "").to_string();
                self.file_icons.as_ref().and_then(|icons| icons.file_icon(&path, &language, self.theme.kind, 13., 22., foreground))
            }
            Some(icon) => Some(self.icon(icon, 16., 16., foreground)),
            None => {
                let key = if self.theme.kind == crate::theme::ThemeKind::Light || item.dto["iconDark"].is_null() { "icon" } else { "iconDark" };
                let path = item.dto[key]["path"].as_str()?.to_owned();
                Some(div().size(px(16.)).flex().items_center().justify_center().child(img(std::path::PathBuf::from(path)).size(px(16.))))
            }
        }
    }

    /// treeView.ts: a click selects the item, then runs its command; an item without one toggles
    /// (`expandOnlyOnTwistieClick` is `!!e.command`).
    fn tree_item_clicked(&mut self, id: &'static str, handle: &str, command: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = self.tree_views.focus.get(id).cloned() {
            window.focus(&focus, cx);
        }
        let tree = self.tree_views.trees.get_mut(id).expect("a drawn row's tree");
        tree.selection = vec![handle.to_owned()];
        tree.focus = Some(handle.to_owned());
        self.host.send("ExtHostTreeViews", "$setSelectionAndFocus", vec![json!(id), json!([handle]), json!(handle)]);
        match command["id"].as_str() {
            Some(command_id) => {
                let args = command["arguments"].as_array().cloned().unwrap_or_default();
                drop(self.execute_command(command_id, args, window, cx));
            }
            None => self.toggle_tree_item(id, handle, cx),
        }
        cx.notify();
    }

    fn toggle_tree_item(&mut self, id: &'static str, handle: &str, cx: &mut Context<Self>) {
        let tree = self.tree_views.trees.get_mut(id).expect("a drawn row's tree");
        let expanded = !tree.expanded.remove(handle);
        if expanded {
            tree.expanded.insert(handle.to_owned());
        }
        self.host.send("ExtHostTreeViews", "$setExpanded", vec![json!(id), json!(handle), json!(expanded)]);
        self.fetch_missing_children(cx);
        cx.notify();
    }
}
