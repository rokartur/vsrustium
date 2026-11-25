//! The Comments panel: commentsView.ts and commentsTreeViewer.ts. Each resource with comments, then each thread
//! with its first comment (icon, author, time, the text's first line, `[Ln n]`) and, with replies, a second line
//! naming the count and the last author. shortcut: no filter box, no sort or show actions, no thread menus, and a
//! markdown body shows its source; upgrade with the comment widget in the editor.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{Context, Div, div, prelude::*, px};
use serde_json::Value;

use super::Workbench;
use super::comments::Thread;
use super::panel::PanelView;
use crate::commands::Outcome;

/// mainThreadComments.ts `COMMENTS_VIEW_ID`.
const COMMENTS_VIEW_ID: &str = "workbench.panel.comments";
const ROW_HEIGHT: f32 = 22.;

/// `ResourceWithCommentThreads`: the threads with comments of one resource, by their controller.
struct Resource<'a> {
    uri: String,
    path: PathBuf,
    owner: Option<&'a str>,
    threads: Vec<(u64, &'a Thread)>,
}

impl Workbench {
    pub(super) fn register_comments_panel_view(&mut self) {
        self.register_panel_view(PanelView {
            id: COMMENTS_VIEW_ID,
            title: "Comments",
            render: |workbench, _, cx| workbench.comments_body(cx).into_any_element(),
            actions: |_, _, _| div().into_any_element(),
            visible: Workbench::comments_view_registered,
            badge: |_| None,
        });
        self.commands.register("workbench.action.focusCommentsPanel", |workbench, _, _, cx| {
            workbench.open_panel_view(COMMENTS_VIEW_ID, cx);
            Outcome::done()
        });
    }

    /// `CommentsModel.resourceCommentThreads`, by `ResourceAscending`: resources by uri, threads by start line.
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
        let mut resources: Vec<Resource<'_>> = by_uri.into_values().collect();
        resources.sort_by(|a, b| a.uri.cmp(&b.uri));
        for resource in &mut resources {
            resource.threads.sort_by_key(|(_, thread)| start_line(thread));
        }
        resources
    }

    fn comments_body(&self, cx: &mut Context<Self>) -> Div {
        let resources = self.comment_resources();
        let list = div().id("comments-tree").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().text_size(px(13.));
        if resources.is_empty() {
            let message = div().pl(px(20.)).line_height(px(22.)).child("There are no comments in this workspace yet.");
            return div().flex_1().min_h_0().flex().flex_col().child(list.child(message));
        }
        let mut rows = Vec::new();
        for resource in &resources {
            let collapsed = self.comments.collapsed.contains(&resource.uri);
            rows.push(self.comments_resource_row(resource, collapsed, cx));
            if !collapsed {
                rows.extend(resource.threads.iter().map(|(controller, thread)| self.comment_thread_row(*controller, thread, cx)));
            }
        }
        div().flex_1().min_h_0().flex().flex_col().child(list.children(rows))
    }

    /// `ResourceWithCommentsRenderer`: the file label, then `· owner` when several controllers have comments.
    fn comments_resource_row(&self, resource: &Resource<'_>, collapsed: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let foreground = self.foreground_or("foreground");
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&resource.path, self.languages.language_of(&resource.path, ""), self.theme.kind, 13., ROW_HEIGHT, foreground));
        let name = resource.path.file_name().map_or_else(|| resource.uri.clone(), |name| name.to_string_lossy().into_owned());
        let description = resource.path.parent().map(|parent| crate::quick_input::path_label(parent, self.workspace())).filter(|label| !label.is_empty());
        let key = resource.uri.clone();
        div()
            .id(gpui::SharedString::from(format!("comments-resource-{key}")))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(8.))
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(self.icon(if collapsed { "chevron-right" } else { "chevron-down" }, 16., 16., foreground).mr(px(2.)))
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().flex_none().child(name))
            .children(description.map(|description| div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(description)))
            .children(resource.owner.map(|owner| div().flex().child(div().px(px(5.)).opacity(0.8).child("\u{b7}")).child(div().text_size(px(11.7)).opacity(0.8).child(owner.to_owned()))))
            .on_click(cx.listener(move |workbench, _, _, cx| {
                if !workbench.comments.collapsed.remove(&key) {
                    workbench.comments.collapsed.insert(key.clone());
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// `CommentNodeRenderer.renderElement`; a click reveals the thread in its file.
    fn comment_thread_row(&self, controller: u64, thread: &Thread, cx: &mut Context<Self>) -> gpui::AnyElement {
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
        div()
            .id(gpui::SharedString::from(format!("comments-thread-{controller}-{thread_handle}")))
            .flex_none()
            .pl(px(30.))
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(first_line)
            .children(replies)
            .on_click(cx.listener(move |workbench, _, window, cx| workbench.reveal_comment_thread(controller, thread_handle, window, cx)))
            .into_any_element()
    }
}

fn start_line(thread: &Thread) -> u64 {
    thread.range.as_ref().and_then(|range| range["startLineNumber"].as_u64()).unwrap_or(0)
}

/// The comment body's first line: a string as it is, a markdown string's source.
fn preview(body: &Value) -> String {
    let text = body.as_str().or(body["value"].as_str()).unwrap_or_default();
    text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().to_owned()
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
}
