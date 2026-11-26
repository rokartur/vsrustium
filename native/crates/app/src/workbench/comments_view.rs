//! The Comments panel: commentsView.ts and commentsTreeViewer.ts. Each resource with comments, then each thread
//! with its first comment (icon, author, time, the text's first line, `[Ln n]`) and, with replies, a second line
//! naming the count and the last author. The title has the filter box with its Show Resolved, Show Unresolved and
//! Sort By menu, and Collapse All. shortcut: no thread menus, no filter highlights or history, and a markdown body
//! shows its source.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Bounds, Context, Div, FocusHandle, KeyDownEvent, Pixels, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::comments::Thread;
use super::input_box_view::{InputId, InputView};
use super::panel::PanelView;
use super::view_filter::ViewFilter;
use crate::commands::Outcome;
use crate::filters::matches_fuzzy;
use crate::input_box::InputBox;
use crate::markers_filter::matches_fuzzy2;
use crate::menus::MenuAction;

/// mainThreadComments.ts `COMMENTS_VIEW_ID`.
const COMMENTS_VIEW_ID: &str = "workbench.panel.comments";
/// commentsView.ts `VIEW_STORAGE_ID` in a `Memento`.
const MEMENTO_KEY: &str = "memento/commentsViewState";
const ROW_HEIGHT: f32 = 22.;

/// The view's filter and tree state (`CommentsFilters`, the filter widget, the tree's focus and selection).
pub(super) struct ViewState {
    pub(super) filter: InputBox,
    pub(super) filter_view: InputView,
    pub(super) filter_focus: FocusHandle,
    filtering: Option<Task<()>>,
    filters: Filters,
    menu_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(super) tree_focus: FocusHandle,
    /// The focused and selected row: a resource's uri or a thread.
    selected: Option<Row>,
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    Resource(String),
    Thread(u64, u64),
}

impl ViewState {
    pub(super) fn new(cx: &mut Context<Workbench>) -> ViewState {
        ViewState {
            filter: InputBox::default(),
            filter_view: InputView::default(),
            filter_focus: cx.focus_handle(),
            filtering: None,
            filters: Filters { text: String::new(), show_resolved: true, show_unresolved: true, sort_by_updated: false },
            menu_bounds: Rc::default(),
            tree_focus: cx.focus_handle(),
            selected: None,
        }
    }
}

/// `CommentsFilters` and the applied filter text, stored in the view's memento.
struct Filters {
    text: String,
    show_resolved: bool,
    show_unresolved: bool,
    /// `CommentsSortOrder.UpdatedAtDescending`, else `ResourceAscending`.
    sort_by_updated: bool,
}

impl Filters {
    fn memento(&self) -> Value {
        let sort_by = if self.sort_by_updated { "updatedAtDescending" } else { "resourceAscending" };
        json!({ "filter": self.text, "showResolved": self.show_resolved, "showUnresolved": self.show_unresolved, "sortBy": sort_by })
    }

    /// commentsTreeViewer.ts `Filter.filterCommentNode`: the resolved state, then the text on the comments' bodies
    /// and authors; `parent_visible` when the resource's name matched.
    fn thread_visible(&self, thread: &Thread, parent_visible: bool) -> bool {
        let state_shown = match thread.state {
            Some(1) => self.show_resolved,
            Some(0) => self.show_unresolved,
            _ => true,
        };
        if !state_shown {
            return false;
        }
        let (negate, text) = filter_text(&self.text);
        if text.is_empty() {
            return true;
        }
        let matched = thread.comments.iter().any(|comment| matches_fuzzy(text, body_text(&comment["body"])).is_some() || matches_fuzzy(text, comment["userName"].as_str().unwrap_or_default()).is_some());
        match (matched, negate, parent_visible) {
            (true, false, _) => true,
            (_, true, false) => !matched,
            _ => parent_visible,
        }
    }
}

/// commentsFilterOptions.ts: a leading `!` negates the trimmed text.
fn filter_text(filter: &str) -> (bool, &str) {
    let filter = filter.trim();
    match filter.strip_prefix('!') {
        Some(rest) => (true, rest.trim()),
        None => (false, filter),
    }
}

/// `ResourceWithCommentThreads`: the threads with comments of one resource, by their controller.
struct Resource<'a> {
    uri: String,
    path: PathBuf,
    owner: Option<&'a str>,
    threads: Vec<(u64, &'a Thread)>,
}

impl Workbench {
    pub(super) fn register_comments_panel_view(&mut self) {
        let stored: Value = self.workspace_storage.lock().unwrap().get(MEMENTO_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        let view = &mut self.comments.view;
        view.filters.show_resolved = stored["showResolved"] != false;
        view.filters.show_unresolved = stored["showUnresolved"] != false;
        view.filters.sort_by_updated = stored["sortBy"] == "updatedAtDescending";
        view.filters.text = stored["filter"].as_str().unwrap_or_default().to_owned();
        view.filter.set_value(view.filters.text.clone());
        self.register_panel_view(PanelView {
            id: COMMENTS_VIEW_ID,
            // mainThreadComments.ts registers the container and its view under the same id.
            view_id: COMMENTS_VIEW_ID,
            title: "Comments",
            open: |workbench, window, cx| {
                workbench.open_panel_view(COMMENTS_VIEW_ID, cx);
                window.focus(&workbench.comments.view.tree_focus, cx);
            },
            render: |workbench, window, cx| {
                let filter = (!workbench.view_filter_in_header(COMMENTS_VIEW_ID)).then(|| workbench.comments_filter_box(window, cx));
                div().flex_1().min_h_0().flex().flex_col().children(filter).child(workbench.comments_body(window, cx)).into_any_element()
            },
            actions: |workbench, window, cx| workbench.comments_actions(window, cx).into_any_element(),
            visible: Workbench::comments_view_registered,
            // comments.contribution.ts `UnresolvedCommentsBadge`: the unresolved threads with comments.
            badge: |workbench| {
                let threads = workbench.comments.controllers.iter().flat_map(|controller| &controller.threads);
                Some(threads.filter(|thread| thread.state == Some(0) && !thread.comments.is_empty()).count())
            },
        });
        self.register_commenting_commands();
        self.register_comment_navigation_commands();
        self.commands.register("workbench.action.focusCommentsPanel", |workbench, _, window, cx| {
            workbench.open_panel_view(COMMENTS_VIEW_ID, cx);
            workbench.select_first_comment_thread();
            window.focus(&workbench.comments.view.tree_focus, cx);
            Outcome::done()
        });
        self.commands.register("workbench.actions.workbench.panel.comments.focusFilter", |workbench, _, window, cx| {
            window.focus(&workbench.comments.view.filter_focus, cx);
            Outcome::done()
        });
        self.commands.register("comments.collapse", |workbench, _, _, cx| {
            workbench.comments.view.selected = None;
            workbench.comments.collapsed = workbench.comment_resources().into_iter().map(|resource| resource.uri).collect();
            cx.notify();
            Outcome::done()
        });
        self.commands.register("comments.expand", |workbench, _, _, cx| {
            workbench.comments.collapsed.clear();
            cx.notify();
            Outcome::done()
        });
        let toggles: [(&str, fn(&mut ViewState)); 4] = [
            ("toggleUnResolvedComments", |view| view.filters.show_unresolved = !view.filters.show_unresolved),
            ("toggleResolvedComments", |view| view.filters.show_resolved = !view.filters.show_resolved),
            ("toggleSortByUpdatedAt", |view| view.filters.sort_by_updated = true),
            ("toggleSortByResource", |view| view.filters.sort_by_updated = false),
        ];
        for (name, toggle) in toggles {
            self.commands.register(format!("workbench.actions.{COMMENTS_VIEW_ID}.{name}"), move |workbench, _, _, cx| {
                toggle(&mut workbench.comments.view);
                workbench.comments_view_state_changed(cx);
                Outcome::done()
            });
        }
    }

    /// `saveState`: the filters in the workspace memento.
    fn comments_view_state_changed(&mut self, cx: &mut Context<Self>) {
        self.workspace_storage.lock().unwrap().set(MEMENTO_KEY, self.comments.view.filters.memento().to_string());
        cx.notify();
    }

    /// `FilterWidget.onDidChangeFilterText` after its 300 ms delay: the filter text applies.
    pub(super) fn edit_comments_filter(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let before = self.comments.view.filter.value.clone();
        edit(&mut self.comments.view.filter);
        cx.notify();
        if self.comments.view.filter.value == before {
            return;
        }
        self.comments.view.filtering = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.comments.view.filters.text = workbench.comments.view.filter.value.clone();
                workbench.comments_view_state_changed(cx);
            });
        }));
    }

    /// commentsView.ts `refresh`: with nothing selected, the first thread is focused and selected.
    pub(super) fn select_first_comment_thread(&mut self) {
        if self.comments.view.selected.is_some() {
            return;
        }
        let first = self.comment_resources().into_iter().next().and_then(|resource| resource.threads.first().map(|(controller, thread)| Row::Thread(*controller, thread.handle)));
        self.comments.view.selected = first;
    }

    /// The title's navigation group: the filter (with Show Resolved, Show Unresolved and Sort By), then Collapse
    /// All while a resource is expanded, else Expand All.
    fn comments_actions(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let resources = self.comment_resources();
        let some_expanded = resources.iter().any(|resource| !self.comments.collapsed.contains(&resource.uri));
        let (icon, command) = if some_expanded { ("collapse-all", "comments.collapse") } else { ("expand-all", "comments.expand") };
        div()
            .flex()
            .items_center()
            .when(self.view_filter_in_header(COMMENTS_VIEW_ID), |actions| actions.child(self.comments_filter_box(window, cx)))
            .when(!resources.is_empty(), |actions| actions.child(self.search_action(icon, command, true, cx)))
    }

    fn comments_filter_box(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let resources = self.comment_resources();
        let total: usize = self.comments.controllers.iter().flat_map(|controller| &controller.threads).filter(|thread| !thread.comments.is_empty()).count();
        let shown: usize = resources.iter().map(|resource| resource.threads.len()).sum();
        let view = &self.comments.view;
        self.view_filter_box(
            ViewFilter {
                container: COMMENTS_VIEW_ID,
                input: InputId::CommentsFilter,
                focused: view.filter_focus.is_focused(window),
                placeholder: "Filter (e.g. text, author)",
                more_filters_checked: !view.filters.show_resolved || !view.filters.show_unresolved,
                badge: (total != shown && total != 0).then(|| format!("Showing {shown} of {total}")),
                menu_bounds: view.menu_bounds.clone(),
                show_menu: Workbench::show_comments_filter_menu,
            },
            cx,
        )
    }

    /// `viewFilterSubmenu` of the Comments view: the two state filters, then the Sort By submenu.
    fn show_comments_filter_menu(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let view = &self.comments.view;
        let item = |name: &str, title: &str, checked: bool| MenuAction::Command { id: format!("workbench.actions.{COMMENTS_VIEW_ID}.{name}"), title: title.into(), enabled: true, checked, alt: None };
        let sort = vec![item("toggleSortByResource", "Position in File", !view.filters.sort_by_updated), item("toggleSortByUpdatedAt", "Updated Time", view.filters.sort_by_updated)];
        let actions = vec![
            item("toggleUnResolvedComments", "Show Unresolved", view.filters.show_unresolved),
            item("toggleResolvedComments", "Show Resolved", view.filters.show_resolved),
            MenuAction::Separator,
            MenuAction::Submenu { title: "Sort By".into(), actions: sort, checked: false },
        ];
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// `CommentsModel.resourceCommentThreads` through the view's filter and sorter: by uri and start line, or by
    /// the last update, newest first.
    fn comment_resources(&self) -> Vec<Resource<'_>> {
        let several_owners = self.comments.controllers.len() > 1;
        let mut by_uri: HashMap<String, Resource<'_>> = HashMap::new();
        for controller in &self.comments.controllers {
            for thread in controller.threads.iter().filter(|thread| !thread.comments.is_empty()) {
                let Ok((scheme, path)) = super::host_requests::uri_parts(&thread.resource) else { continue };
                let uri = crate::editor::uri_string(scheme, &path);
                let resource = by_uri.entry(format!("{}\u{0}{uri}", controller.handle)).or_insert_with(|| Resource { uri, path, owner: several_owners.then_some(controller.label.as_str()), threads: Vec::new() });
                resource.threads.push((controller.handle, thread));
            }
        }
        let view = &self.comments.view;
        let (negate, text) = filter_text(&view.filters.text);
        let mut resources: Vec<Resource<'_>> = by_uri.into_values().collect();
        for resource in &mut resources {
            let name = resource.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let name_matched = !text.is_empty() && !negate && matches_fuzzy2(text, &name).is_some();
            resource.threads.retain(|(_, thread)| view.filters.thread_visible(thread, name_matched));
        }
        resources.retain(|resource| !resource.threads.is_empty());
        match view.filters.sort_by_updated {
            true => {
                for resource in &mut resources {
                    resource.threads.sort_by(|a, b| last_updated(b.1).cmp(last_updated(a.1)));
                }
                resources.sort_by(|a, b| last_updated(b.threads[0].1).cmp(last_updated(a.threads[0].1)));
            }
            false => {
                resources.sort_by(|a, b| a.uri.cmp(&b.uri));
                for resource in &mut resources {
                    resource.threads.sort_by_key(|(_, thread)| start_line(thread));
                }
            }
        }
        resources
    }

    /// The tree's rows in order, as Up and Down walk them.
    fn comment_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for resource in self.comment_resources() {
            let collapsed = self.comments.collapsed.contains(&resource.uri);
            rows.push(Row::Resource(resource.uri));
            if !collapsed {
                rows.extend(resource.threads.iter().map(|(controller, thread)| Row::Thread(*controller, thread.handle)));
            }
        }
        rows
    }

    fn comments_tree_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.comment_rows();
        let index = self.comments.view.selected.as_ref().and_then(|selected| rows.iter().position(|row| row == selected));
        let next = match event.keystroke.key.as_str() {
            "down" => index.map_or(0, |index| (index + 1).min(rows.len().saturating_sub(1))),
            "up" => index.map_or(0, |index| index.saturating_sub(1)),
            "enter" | "space" => {
                match self.comments.view.selected.clone() {
                    Some(Row::Thread(controller, thread)) => self.reveal_comment_thread(controller, thread, window, cx),
                    Some(Row::Resource(uri)) => self.toggle_comments_resource(uri, cx),
                    None => {}
                }
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.comments.view.selected = rows.get(next).cloned();
        cx.notify();
    }

    fn toggle_comments_resource(&mut self, uri: String, cx: &mut Context<Self>) {
        if !self.comments.collapsed.remove(&uri) {
            self.comments.collapsed.insert(uri);
        }
        cx.notify();
    }

    fn comments_body(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let resources = self.comment_resources();
        let focused = self.comments.view.tree_focus.is_focused(window);
        let list = div()
            .id("comments-tree")
            .track_focus(&self.comments.view.tree_focus)
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| workbench.comments_tree_key_down(event, window, cx)))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .text_size(px(13.));
        if resources.is_empty() {
            let message = match self.comments.controllers.iter().any(|controller| controller.threads.iter().any(|thread| !thread.comments.is_empty())) {
                true => "No comments match the provided filter.",
                false => "There are no comments in this workspace yet.",
            };
            let message = div().pl(px(20.)).line_height(px(22.)).child(message);
            return div().flex_1().min_h_0().flex().flex_col().child(list.child(message));
        }
        let mut rows = Vec::new();
        for resource in &resources {
            let collapsed = self.comments.collapsed.contains(&resource.uri);
            rows.push(self.comments_resource_row(resource, collapsed, focused, cx));
            if !collapsed {
                rows.extend(resource.threads.iter().map(|(controller, thread)| self.comment_thread_row(*controller, thread, focused, cx)));
            }
        }
        div().flex_1().min_h_0().flex().flex_col().child(list.children(rows))
    }

    /// The selected row's background and, in the focused tree, its `list.focusOutline`.
    pub(super) fn list_row_selection(&self, row: gpui::Stateful<Div>, selected: bool, tree_focused: bool) -> gpui::Stateful<Div> {
        if !selected {
            return row.hover(|row| row.bg(self.color("list.hoverBackground")));
        }
        let (background, foreground) = match tree_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let outline = self.theme.color("list.focusAndSelectionOutline").or(self.theme.color("list.focusOutline")).filter(|_| tree_focused);
        row.relative()
            .bg(background)
            .when_some(foreground, |row, color| row.text_color(color))
            // The row's own rounding (the Problems rows' 4px) shows only on the background; the outline takes it too.
            .when_some(outline, |mut row, color| {
                let radius = row.style().corner_radii.top_left;
                row.child(div().absolute().inset_0().border_1().border_color(color).when_some(radius, |outline, radius| outline.rounded(radius)))
            })
    }

    /// `ResourceWithCommentsRenderer`: the file label, then `· owner` when several controllers have comments.
    fn comments_resource_row(&self, resource: &Resource<'_>, collapsed: bool, tree_focused: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let foreground = self.foreground_or("foreground");
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&resource.path, self.languages.language_of(&resource.path, ""), self.theme.kind, 13., ROW_HEIGHT, foreground));
        let name = resource.path.file_name().map_or_else(|| resource.uri.clone(), |name| name.to_string_lossy().into_owned());
        let description = resource.path.parent().map(|parent| crate::quick_input::path_label(parent, self.workspace())).filter(|label| !label.is_empty());
        let key = resource.uri.clone();
        let selected = self.comments.view.selected == Some(Row::Resource(key.clone()));
        let row = div()
            .id(gpui::SharedString::from(format!("comments-resource-{key}")))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(8.))
            .cursor_pointer();
        self.list_row_selection(row, selected, tree_focused)
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, 16., 16., foreground).mr(px(2.)))
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().flex_none().child(name))
            .children(description.map(|description| div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(description)))
            .children(resource.owner.map(|owner| div().flex().child(div().px(px(5.)).opacity(0.8).child("\u{b7}")).child(div().text_size(px(11.7)).opacity(0.8).child(owner.to_owned()))))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                workbench.comments.view.selected = Some(Row::Resource(key.clone()));
                window.focus(&workbench.comments.view.tree_focus, cx);
                workbench.toggle_comments_resource(key.clone(), cx);
            }))
            .into_any_element()
    }

    /// `CommentNodeRenderer.renderElement`; a click selects the thread and reveals it in its file.
    fn comment_thread_row(&self, controller: u64, thread: &Thread, tree_focused: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let foreground = self.foreground_or("foreground");
        let first = &thread.comments[0];
        // `getIcon`: a draft comment, then an unresolved thread, else the plain comment icon.
        let draft = thread.comments.iter().any(|comment| comment["state"] == 1);
        let (icon, color) = match (draft, thread.state) {
            (true, _) => ("comment-draft", foreground),
            (false, Some(0)) => ("comment-unresolved", self.color("commentsView.unresolvedIcon")),
            (false, Some(_)) => ("comment", self.color("commentsView.resolvedIcon")),
            (false, None) => ("comment", foreground),
        };
        let now = chrono::Utc::now().timestamp_millis();
        let range = thread.range.as_ref().map(|range| match (range["startLineNumber"].as_u64(), range["endLineNumber"].as_u64()) {
            (Some(start), Some(end)) if start != end => format!("[Ln {start}-{end}]"),
            (start, _) => format!("[Ln {}]", start.unwrap_or(1)),
        });
        let first_line = div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .mr(px(5.))
            .child(self.icon(icon, 16., 16., color).mr(px(5.)))
            .child(div().flex_none().pr(px(5.)).child(first["userName"].as_str().unwrap_or_default().to_owned()))
            .children(timestamp(first, now).map(|time| div().flex_none().pr(px(5.)).text_size(px(11.7)).opacity(0.8).child(time)))
            .child(div().flex_none().pr(px(5.)).opacity(0.8).child("\u{b7}"))
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(preview(&first["body"])))
            .children(range.map(|range| div().flex_none().pr(px(5.)).opacity(0.8).child(range)));
        let replies = (thread.comments.len() > 1).then(|| {
            let last = &thread.comments[thread.comments.len() - 1];
            let count = match thread.comments.len() {
                2 => "1 reply".to_owned(),
                count => format!("{} replies", count - 1),
            };
            div()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .pl(px(16.))
                .mr(px(5.))
                .child(self.icon("indent", 16., 16., foreground).mr(px(5.)))
                .child(div().flex_none().pr(px(5.)).child(count))
                .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().pr(px(5.)).text_size(px(11.7)).opacity(0.8).child(format!("Last reply from {}", last["userName"].as_str().unwrap_or_default())))
                .child(div().flex_none().pr(px(5.)).opacity(0.8).child("\u{b7}"))
                .children(timestamp(last, now).map(|time| div().text_size(px(11.7)).opacity(0.8).child(time)))
        });
        let thread_handle = thread.handle;
        let selected = self.comments.view.selected == Some(Row::Thread(controller, thread_handle));
        let row = div().id(gpui::SharedString::from(format!("comments-thread-{controller}-{thread_handle}"))).flex_none().pl(px(30.)).cursor_pointer();
        self.list_row_selection(row, selected, tree_focused)
            .child(first_line)
            .children(replies)
            .on_click(cx.listener(move |workbench, _, window, cx| {
                workbench.comments.view.selected = Some(Row::Thread(controller, thread_handle));
                workbench.reveal_comment_thread(controller, thread_handle, window, cx);
            }))
            .into_any_element()
    }
}

fn start_line(thread: &Thread) -> u64 {
    thread.range.as_ref().and_then(|range| range["startLineNumber"].as_u64()).unwrap_or(0)
}

/// commentModel.ts `CommentNode.lastUpdatedAt`: the newest of the comments' timestamps, compared as strings.
fn last_updated(thread: &Thread) -> &str {
    thread.comments.iter().filter_map(|comment| comment["timestamp"].as_str()).max().unwrap_or_default()
}

fn body_text(body: &Value) -> &str {
    body.as_str().or(body["value"].as_str()).unwrap_or_default()
}

/// The comment body's first line: a string as it is, a markdown string's source.
fn preview(body: &Value) -> String {
    body_text(body).lines().find(|line| !line.trim().is_empty()).unwrap_or_default().to_owned()
}
/// timestamp.ts with `comments.useRelativeTime`: `fromNow(date, true, true)`.
fn timestamp(comment: &Value, now_ms: i64) -> Option<String> {
    let time = chrono::DateTime::parse_from_rfc3339(comment["timestamp"].as_str()?).ok()?;
    Some(from_now_full_words(time.timestamp_millis(), now_ms))
}

/// date.ts `fromNow(date, true, true)`: the relative time in full words with " ago".
fn from_now_full_words(timestamp_ms: i64, now_ms: i64) -> String {
    let short = crate::timeline::from_now_ago(timestamp_ms, now_ms);
    let words = [("secs", "seconds"), ("sec", "second"), ("mins", "minutes"), ("min", "minute"), ("hrs", "hours"), ("hr", "hour"), ("wks", "weeks"), ("wk", "week"), ("mos", "months"), ("mo", "month"), ("yrs", "years"), ("yr", "year")];
    let mut parts: Vec<&str> = short.split(' ').collect();
    for part in &mut parts {
        if let Some((_, full)) = words.iter().find(|(word, _)| word == part) {
            *part = full;
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_use_full_words() {
        let now = 1_000_000_000_000;
        assert_eq!(from_now_full_words(now - 3 * 60_000, now), "3 minutes ago");
        assert_eq!(from_now_full_words(now - 3_600_000, now), "1 hour ago");
        assert_eq!(from_now_full_words(now - 2 * 86_400_000, now), "2 days ago");
        assert_eq!(from_now_full_words(now, now), "now");
        assert_eq!(preview(&serde_json::json!({ "value": "\n**Bold** start\nmore" })), "**Bold** start");
    }

    /// commentsTreeViewer.ts `Filter`: the resolved state first, then the text on bodies and authors, `!` negating
    /// it unless the resource's name matched.
    #[test]
    fn the_filter_takes_the_state_then_the_text() {
        let thread = |state: Option<u64>, body: &str, author: &str| Thread {
            handle: 1,
            resource: Value::Null,
            range: None,
            label: None,
            context_value: None,
            comments: vec![json!({ "body": body, "userName": author })],
            collapse_state: 0,
            revealed: false,
            can_reply: json!(true),
            state,
            is_template: false,
        };
        let filters = |text: &str, show_resolved: bool| Filters { text: text.into(), show_resolved, show_unresolved: true, sort_by_updated: false };
        let resolved = thread(Some(1), "Rename total", "Cid");
        assert!(filters("", true).thread_visible(&resolved, false));
        assert!(!filters("", false).thread_visible(&resolved, false));
        assert!(filters("cid", true).thread_visible(&resolved, false));
        assert!(filters("renam", true).thread_visible(&resolved, false));
        assert!(!filters("ann", true).thread_visible(&resolved, false));
        assert!(filters("ann", true).thread_visible(&resolved, true));
        assert!(!filters("!cid", true).thread_visible(&resolved, false));
        assert!(filters(" ! ann ", true).thread_visible(&resolved, false));
    }
}
