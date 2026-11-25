//! The Timeline view of the Explorer (timelinePane.ts, timeline.contribution.ts) with mainThreadTimeline.ts: the
//! `MainThreadTimeline` requests, the section following the active editor, its rows and the `timeline.*` commands.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{ClickEvent, Context, Div, SharedString, Stateful, canvas, div, prelude::*, px};
use serde_json::{Map, Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::scm_view::{INDENT, ROW_HEIGHT, TWISTIE};
use super::search_view::SideBarView;
use crate::commands::Outcome;
use crate::timeline::{Row, Timeline, TimelineItem, TimelineProvider, TimelineRequest, TimelineResult};

const EXCLUDE_SOURCES_KEY: &str = "timeline.excludeSources";
/// timeline.ts `TimelineTriggerKind`.
const TRIGGER_INVOKE: u64 = 1;
const TRIGGER_AUTOMATIC: u64 = 2;

#[derive(Default)]
pub struct TimelineView {
    model: Timeline,
    expanded: bool,
    /// `followActiveEditor` turned off ("Pin Current Timeline").
    pinned: bool,
    /// The body was visible on the last render; becoming visible runs `onActiveEditorChanged` again.
    shown: bool,
    /// The active editor's resource the view last followed.
    followed: Option<Value>,
    selected: Option<String>,
}

impl Workbench {
    /// timeline.contribution.ts and `TimelinePaneCommands`: refresh and pin.
    pub(in crate::workbench) fn register_timeline_commands(&mut self) {
        self.commands.register("timeline.refresh", |workbench, _, _, cx| {
            workbench.load_timeline(true, None, TRIGGER_INVOKE, cx);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("timeline.toggleFollowActiveEditor", |workbench, _, _, cx| {
            workbench.timeline.pinned = !workbench.timeline.pinned;
            workbench.timeline.shown = false;
            cx.notify();
            Outcome::done()
        });
    }

    /// One `MainThreadTimeline` request.
    pub(crate) fn handle_timeline_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let id = args.first().and_then(Value::as_str).unwrap_or_default().to_string();
        match method {
            "$registerTimelineProvider" => {
                let Some(provider) = args.first().and_then(TimelineProvider::from_descriptor) else { return Outcome::done() };
                let id = provider.id.clone();
                if self.timeline.model.providers.iter().all(|other| other.id != id) {
                    self.register_exclude_source_command(&id);
                }
                self.timeline.model.register(provider);
                self.load_timeline(true, Some(vec![id]), TRIGGER_AUTOMATIC, cx);
            }
            "$unregisterTimelineProvider" => {
                if self.timeline.model.providers.iter().any(|provider| provider.id == id) {
                    self.commands.unregister(&format!("timeline.toggleExcludeSource:{id}"));
                }
                self.timeline.model.unregister(&id);
            }
            "$emitTimelineChangeEvent" => self.timeline_changed(&id, args.get(1), cx),
            _ => return Outcome::done(),
        }
        cx.notify();
        Outcome::done()
    }

    /// `isBodyVisible`: expanded, in the Explorer on screen.
    fn timeline_visible(&self) -> bool {
        self.timeline.expanded && self.side_bar_view == SideBarView::Explorer && self.side_bar_visible()
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
        let target = self.timeline_target();
        if became_visible || self.timeline.followed != target {
            self.timeline.followed = target.clone();
            self.follow_timeline_target(target, cx);
        }
    }

    /// The active editor's resource, without the schemes that have no timeline.
    fn timeline_target(&self) -> Option<Value> {
        let editor = self.editors.active()?;
        let excluded = ["vscode-settings", "webview-panel", "walkThrough"];
        (!excluded.contains(&editor.document.scheme.as_str())).then(|| crate::editor::uri_value(&editor.document.scheme, &editor.document.path))
    }

    /// The rest of `onActiveEditorChanged`: the same resource only loads missing or stale sources (`reloadTimeline`).
    fn follow_timeline_target(&mut self, target: Option<Value>, cx: &mut Context<Self>) {
        let same = match (&target, &self.timeline.model.uri) {
            (Some(target), Some(shown)) => same_resource(target, shown),
            (target, shown) => target.is_none() && shown.is_none(),
        };
        if !same {
            self.timeline.model.uri = target;
            self.timeline.selected = None;
            return self.load_timeline(true, None, TRIGGER_AUTOMATIC, cx);
        }
        let excluded = self.timeline_excluded_sources();
        let page_size = self.timeline_page_size();
        let sources: Vec<String> = self.timeline.model.providers.iter().map(|provider| provider.id.clone()).filter(|id| !excluded.contains(id)).collect();
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

    /// timelinePane.ts `loadTimeline`: every source, or `sources`, asked for the shown resource.
    fn load_timeline(&mut self, reset: bool, sources: Option<Vec<String>>, trigger: u64, cx: &mut Context<Self>) {
        let page_size = self.timeline_page_size();
        if sources.is_none() && reset {
            self.timeline.model.clear(true, page_size);
        }
        if self.timeline.model.uri.is_none() {
            return self.timeline.model.clear(false, page_size);
        }
        if !self.timeline_visible() {
            return;
        }
        let excluded = self.timeline_excluded_sources();
        let sources = sources.unwrap_or_else(|| self.timeline.model.providers.iter().map(|provider| provider.id.clone()).collect());
        for source in sources {
            if let Some(request) = self.timeline.model.request(&source, reset, None, page_size, &excluded) {
                self.send_timeline_request(request, trigger, cx);
            }
        }
    }

    /// mainThreadTimeline.ts `provideTimeline`: `ExtHostTimeline.$getTimeline(id, uri, options, token)`, its answer
    /// handled as `handleRequest` does.
    fn send_timeline_request(&mut self, request: TimelineRequest, trigger: u64, cx: &mut Context<Self>) {
        let Some(uri) = self.timeline.model.uri.clone() else { return };
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

    /// `onTimelineChanged`: a change of the shown resource (or of every resource) reloads that source, or marks it
    /// stale while the body is hidden.
    fn timeline_changed(&mut self, source: &str, uris: Option<&Value>, cx: &mut Context<Self>) {
        let Some(shown) = &self.timeline.model.uri else { return };
        let affected = uris.and_then(Value::as_array).is_none_or(|uris| uris.iter().any(|uri| same_resource(uri, shown)));
        if !affected || self.timeline.model.timeline(source).is_none() {
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

    /// The sources "Filter Timeline" unchecked, `timeline.excludeSources` in the profile storage.
    fn timeline_excluded_sources(&self) -> HashSet<String> {
        let stored = self.profile_storage.lock().unwrap().get(EXCLUDE_SOURCES_KEY).map(str::to_owned);
        stored.and_then(|stored| serde_json::from_str::<Vec<String>>(&stored).ok()).unwrap_or_default().into_iter().collect()
    }

    /// `updateTimelineSourceFilters`: `timeline.toggleExcludeSource:<id>`, then `onStorageServiceChanged`.
    fn register_exclude_source_command(&mut self, source: &str) {
        let source = source.to_string();
        self.commands.register(format!("timeline.toggleExcludeSource:{source}"), move |workbench, _, _, cx| {
            let mut excluded = workbench.timeline_excluded_sources();
            if !excluded.remove(&source) {
                excluded.insert(source.clone());
            }
            let mut stored: Vec<&String> = excluded.iter().collect();
            stored.sort();
            workbench.profile_storage.lock().unwrap().set(EXCLUDE_SOURCES_KEY, json!(stored).to_string());
            let missing: Vec<String> = workbench.timeline.model.providers.iter().map(|provider| provider.id.clone()).filter(|id| !excluded.contains(id) && workbench.timeline.model.timeline(id).is_none()).collect();
            if !missing.is_empty() {
                workbench.load_timeline(true, Some(missing), TRIGGER_AUTOMATIC, cx);
            }
            cx.notify();
            Outcome::done()
        });
    }

    /// The Timeline pane of the Explorer: its header, and when expanded a share of the height with the message or
    /// the rows (timelinePane.css).
    pub(in crate::workbench) fn timeline_section(&self, cx: &mut Context<Self>) -> Div {
        let expanded = self.timeline.expanded;
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let label = self.timeline.model.uri.as_ref().and_then(|uri| uri["path"].as_str()).and_then(|path| path.rsplit('/').next()).unwrap_or_default().to_string();
        // timelinePane.ts `updateFilename`.
        let description = match self.timeline.pinned {
            true if !label.is_empty() => format!("{label} (pinned)"),
            _ => label.clone(),
        };
        let header = self
            .section_header("Timeline", expanded, true)
            .id("timeline-header")
            .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| {
                workbench.timeline.expanded = !workbench.timeline.expanded;
                cx.notify();
            }))
            .when(expanded, |header| {
                header
                    .child(div().flex_1().min_w_0().ml(px(10.)).overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(self.color("panelTitle.inactiveForeground")).child(description))
                    .child(
                        div()
                            .flex()
                            .pr(px(8.))
                            .invisible()
                            .group_hover("timeline-pane", |actions| actions.visible())
                            .child(self.timeline_action("refresh", "Refresh", "timeline.refresh", foreground, cx))
                            .child(match self.timeline.pinned {
                                true => self.timeline_action("pinned", "Unpin Current Timeline", "timeline.toggleFollowActiveEditor", foreground, cx),
                                false => self.timeline_action("pin", "Pin Current Timeline", "timeline.toggleFollowActiveEditor", foreground, cx),
                            }),
                    )
            });
        let section = div().flex().flex_col().group("timeline-pane").child(header);
        if !expanded {
            return section.flex_none();
        }
        let excluded = self.timeline_excluded_sources();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |now| now.as_millis() as i64);
        let message = self.timeline.model.message(&excluded, &label, self.scm.model.providers.len());
        let rows: Vec<Stateful<Div>> = self
            .timeline
            .model
            .rows(&excluded, now)
            .into_iter()
            .map(|row| match row {
                Row::Item { item, relative_time, duplicate } => self.timeline_item_row(item, relative_time, duplicate, cx),
                Row::LoadMore { loading } => self.timeline_load_more_row(loading, cx),
            })
            .collect();
        let message = message.map(|message| div().absolute().top_0().left_0().right_0().pt(px(10.)).px(px(22.)).opacity(0.5).child(message));
        section.flex_1().min_h_0().child(div().relative().flex_1().min_h_0().flex().flex_col().children(message).child(div().id("timeline-tree").flex_1().min_h_0().overflow_y_scroll().children(rows)))
    }

    fn timeline_action(&self, icon: &str, title: &str, command: &'static str, foreground: gpui::Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        self.icon(icon, 22., 16., foreground)
            .id(SharedString::from(format!("timeline-action-{icon}")))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|action| action.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                drop(workbench.execute_command(command, Vec::new(), window, cx));
            }))
            .child(self.hover_target(SharedString::from(format!("timeline-action-hover-{icon}")), HoverContent::Text(title.to_string()), HoverOptions::mouse()))
    }

    /// `TimelineTreeRenderer.renderElement`: icon, label and description, the relative time; a time equal to the one
    /// above shows as a faint line until the row is hovered or selected.
    fn timeline_item_row(&self, item: &TimelineItem, relative_time: String, duplicate: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let id = SharedString::from(format!("timeline-item-{}", item.handle));
        let selected = self.timeline.selected.as_deref() == Some(item.handle.as_str());
        let duplicate = duplicate && !selected;
        let icon = item.theme_icon.as_ref().map(|(name, color)| self.icon(name, 16., 16., color.as_deref().and_then(|color| self.theme.color(color)).unwrap_or(foreground)));
        let handle = item.handle.clone();
        let command = item.command.clone();
        let timestamp = div()
            .flex_none()
            .ml(px(2.))
            .mr(px(4.))
            .opacity(0.5)
            .overflow_hidden()
            .whitespace_nowrap()
            .child(div().when(duplicate, |time| time.invisible().group_hover(id.clone(), |time| time.visible())).child(relative_time))
            .when(duplicate, |container| container.w(px(10.)).group_hover(id.clone(), |container| container.w_auto()));
        self.timeline_row(id.clone(), selected)
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                workbench.timeline.selected = Some(handle.clone());
                cx.notify();
                if let Some(command) = &command
                    && let Some(id) = command["id"].as_str()
                {
                    let args = command["arguments"].as_array().cloned().unwrap_or_default();
                    drop(workbench.execute_command(id, args, window, cx));
                }
            }))
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
            .when(duplicate, |row| row.child(div().absolute().top_0().bottom_0().right(px(10.)).w(px(1.)).bg(foreground).opacity(0.125).group_hover(id.clone(), |line| line.invisible())))
            .children(item.tooltip.clone().map(|tooltip| self.hover_target(SharedString::from(format!("timeline-hover-{}", item.handle)), HoverContent::Text(tooltip), HoverOptions::instant())))
    }

    /// `LoadMoreCommand`: "Load more", "Loading..." while a request is out; with `timeline.pageOnScroll` it loads as
    /// soon as it is painted in view (`onDidScrollToEnd`).
    fn timeline_load_more_row(&self, loading: bool, cx: &mut Context<Self>) -> Stateful<Div> {
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
        self.timeline_row("timeline-load-more".into(), false)
            .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| workbench.load_more_timeline(cx)))
            .child(div().w(px(22.)).flex_none())
            .child(if loading { "Loading..." } else { "Load more" })
            .when(page_on_scroll && !loading, |row| row.child(scrolled_to_end))
    }

    /// views.css `.custom-view-tree-node-item`: a 22px row, a leaf of a tree without twisties.
    fn timeline_row(&self, id: SharedString, selected: bool) -> Stateful<Div> {
        div()
            .id(id.clone())
            .group(id)
            .relative()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .pl(px(INDENT + TWISTIE))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
    }
}

/// `extUri.isEqual`, or for `file` and `git` resources the same `fsPath`.
fn same_resource(a: &Value, b: &Value) -> bool {
    let part = |uri: &Value, key: &str| uri[key].as_str().unwrap_or_default().to_string();
    let same = |key: &str| part(a, key) == part(b, key);
    let file_like = |uri: &Value| matches!(uri["scheme"].as_str(), Some("file" | "git"));
    (same("scheme") && same("authority") && same("path") && same("query")) || (same("path") && file_like(a) && file_like(b))
}
