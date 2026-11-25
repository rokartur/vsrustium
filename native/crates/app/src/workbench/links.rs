//! links.ts, getLinks.ts and links.css: the providers' links of the active model, underlined; with the
//! multi-cursor modifier's opposite (Cmd with `editor.multiCursorModifier` alt) a link under the pointer turns
//! `editorLink.activeForeground` with a pointer cursor and a click opens it (`ClickLinkGesture`).

use gpui::{Context, HighlightStyle, Hsla, Modifiers, Task, UnderlineStyle, Window, px};
use serde_json::{Value, json};
use std::ops::Range;
use std::time::Duration;

use super::Workbench;
use super::content_hover::{Anchor, Part, PartContent, Participant};
use super::opener::{ContributedOpeners, OpenOptions, Target};
use crate::commands::Outcome;
use crate::editor::{ContentChanged, OpenEditor, Position, char_column, expand_tabs, utf16_column};
use crate::language_features::Candidate;
use crate::link_computer::compute_links;
use crate::notifications::{Notification, Severity};

const REGISTER: &str = "$registerDocumentLinkProvider";
/// The editor worker's link provider (`editorWorkerService.ts` registers `computeLinks` for every model).
pub(super) const WORKER_LINK_PROVIDER: u64 = u64::MAX - 3;
/// `languageFeatureDebounceService.for(linkProvider, 'Links', { min: 1000, max: 4000 })`.
const DEBOUNCE: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub(super) struct Links {
    model_id: Option<u32>,
    pub(super) list: Vec<Link>,
    /// The providers' lists to release once replaced (`LinksList.dispose`).
    caches: Vec<(u64, u64)>,
    compute: Option<Task<()>>,
    /// The link under the pointer while the trigger modifier is down (`activeLinkDecorationId`).
    active: Option<usize>,
    /// The pointer's text position (1-based line, UTF-16 column) and modifiers of the last move over the text.
    pointer: Option<(usize, usize)>,
    modifiers: Modifiers,
}

/// A `Link` of `getLinks.ts`: the provider's `ILink` with its range tracked through edits, 1-based, UTF-16 columns.
#[derive(Clone, Debug)]
pub(super) struct Link {
    handle: u64,
    dto: Value,
    start: (usize, usize),
    end: (usize, usize),
}

impl Workbench {
    pub(super) fn register_links(&mut self) {
        self.language_features.register(REGISTER, vec![json!(WORKER_LINK_PROVIDER), json!({ "language": "*", "hasAccessToAllModels": true }), json!(false)]);
        self.commands.register("editor.action.openLink", |workbench, _, window, cx| {
            workbench.open_links_at_selections(window, cx);
            Outcome::done()
        });
        self.commands.register("_executeLinkProvider", |workbench, args, _, cx| workbench.execute_link_provider(&args, cx));
    }

    /// `computeLinks.schedule(0)` on another model, a language change, a provider change or `editor.links`.
    pub(super) fn links_model_changed(&mut self, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if self.links.model_id != model_id {
            self.release_links();
            self.links.model_id = model_id;
        }
        self.schedule_links(Duration::ZERO, cx);
    }

    /// `onDidChangeModelContent`: the links follow the edit (`NeverGrowsWhenTypingAtEdges`), then are asked again
    /// after the debounce.
    pub(super) fn links_content_changed(&mut self, change: &ContentChanged, cx: &mut Context<Self>) {
        for content in &change.changes {
            let start = (content.start_line, content.start_column);
            let end = (content.end_line, content.end_column);
            let new_end = end_after(start, &content.text);
            for link in &mut self.links.list {
                link.start = tracked(link.start, start, end, new_end, false);
                link.end = tracked(link.end, start, end, new_end, true);
            }
        }
        self.schedule_links(DEBOUNCE, cx);
    }

    fn schedule_links(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.links.compute = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(request) = workbench.update(cx, |workbench, _| workbench.request_links()) else { return };
            let Some((model_id, version_id, calls)) = request else { return };
            let mut lists = Vec::new();
            for (handle, call) in calls {
                if let Ok(Ok(list)) = call.await
                    && list.is_object()
                {
                    lists.push((handle, list));
                }
            }
            let _ = workbench.update(cx, |workbench, cx| workbench.links_answered(model_id, version_id, lists, cx));
        }));
    }

    /// `computeLinksNow`: every provider of the model, newest first (`ordered(model).reverse()`).
    #[allow(clippy::type_complexity)]
    fn request_links(&mut self) -> Option<(u32, u32, Vec<(u64, futures::channel::oneshot::Receiver<Result<Value, Value>>)>)> {
        let editor = self.editors.active()?;
        if !self.links_enabled(editor) {
            return None;
        }
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().rev().map(|registration| registration.handle).collect();
        let lines = editor.document.lines.clone();
        let calls = handles
            .into_iter()
            .map(|handle| match handle {
                WORKER_LINK_PROVIDER => {
                    let (sender, receiver) = futures::channel::oneshot::channel();
                    let _ = sender.send(Ok(worker_links(&lines)));
                    (handle, receiver)
                }
                _ => (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentLinks", vec![json!(handle), uri.clone()])),
            })
            .collect();
        Some((editor.model_id, editor.version_id, calls))
    }

    fn links_enabled(&self, editor: &OpenEditor) -> bool {
        let overrides = config::configuration::Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        self.settings.lock().unwrap().get_value(Some("editor.links"), &overrides) != Some(json!(false))
    }

    /// `LinksList`: each provider's links joined by `_union`, which drops a later list's links that touch earlier ones.
    fn links_answered(&mut self, model_id: u32, version_id: u32, lists: Vec<(u64, Value)>, cx: &mut Context<Self>) {
        let current = self.editors.active().is_some_and(|editor| (editor.model_id, editor.version_id) == (model_id, version_id));
        if !current {
            for (handle, list) in &lists {
                self.release_link_list(*handle, list);
            }
            return;
        }
        self.release_links();
        let mut links: Vec<Link> = Vec::new();
        for (handle, list) in &lists {
            let new_links: Vec<Link> = list["links"].as_array().into_iter().flatten().map(|dto| link_from_dto(*handle, dto)).collect();
            links = union(links, new_links);
        }
        self.links.list = links;
        self.links.caches = lists.iter().filter_map(|(handle, list)| Some((*handle, list["cacheId"].as_u64()?))).collect();
        self.links.active = None;
        self.update_active_link(cx);
        cx.notify();
    }

    fn release_links(&mut self) {
        for (handle, cache_id) in std::mem::take(&mut self.links.caches) {
            self.host.send("ExtHostLanguageFeatures", "$releaseDocumentLinks", vec![json!(handle), json!(cache_id)]);
        }
        self.links.list.clear();
        self.links.active = None;
    }

    fn release_link_list(&self, handle: u64, list: &Value) {
        if let Some(cache_id) = list["cacheId"].as_u64() {
            self.host.send("ExtHostLanguageFeatures", "$releaseDocumentLinks", vec![json!(handle), json!(cache_id)]);
        }
    }

    /// `ClickLinkGesture.onMouseMoveOrRelevantKeyDown`: the pointer's line and visible column, `None` off the lines;
    /// only a char of the text counts (`MouseTargetType.CONTENT_TEXT`).
    pub(super) fn links_mouse_moved(&mut self, target: Option<(usize, f32)>, modifiers: Modifiers, cx: &mut Context<Self>) {
        self.links.pointer = self.editors.active().zip(target).and_then(|(editor, (line, visible))| {
            let anchor = super::content_hover::anchor_at(editor, line, visible).filter(|anchor| anchor.start < anchor.end)?;
            Some((line + 1, utf16_column(&editor.document.lines[line], anchor.start) + 1))
        });
        self.links.modifiers = modifiers;
        self.update_active_link(cx);
    }

    pub(super) fn links_modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        self.links.modifiers = modifiers;
        self.update_active_link(cx);
    }

    /// `_onEditorMouseMove`: one active link at most, under the pointer while the trigger modifier is down.
    fn update_active_link(&mut self, cx: &mut Context<Self>) {
        let active = match self.links.pointer.filter(|_| self.link_trigger_modifier(self.links.modifiers)) {
            Some(position) => self.link_index_at(position),
            None => None,
        };
        if active != self.links.active {
            self.links.active = active;
            cx.notify();
        }
    }

    /// `hasTriggerModifier`: Cmd when the multi-cursor modifier is Alt, else Alt, and no other modifier counts.
    fn link_trigger_modifier(&self, modifiers: Modifiers) -> bool {
        match self.multi_cursor_modifier_is_cmd() {
            true => modifiers.alt,
            false => modifiers.platform,
        }
    }

    /// `getLinkOccurrence`: the link whose range holds the position, edges included.
    fn link_index_at(&self, (line, column): (usize, usize)) -> Option<usize> {
        self.links.list.iter().position(|link| link.start <= (line, column) && (line, column) <= link.end)
    }

    /// `onExecute`: a click with the trigger modifier, or a middle click with `editor.mouseMiddleClickAction`
    /// `openLink`, on a link opens it. Answers whether it took the click.
    pub(super) fn links_mouse_up(&mut self, line: usize, visible: f32, modifiers: Modifiers, middle: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let triggered = match middle {
            true => self.setting("editor.mouseMiddleClickAction") == json!("openLink"),
            false => self.link_trigger_modifier(modifiers),
        };
        let Some(editor) = self.editors.active().filter(|_| triggered) else { return false };
        let Some(anchor) = super::content_hover::anchor_at(editor, line, visible).filter(|anchor| anchor.start < anchor.end) else { return false };
        let at = (line + 1, utf16_column(&editor.document.lines[line], anchor.start) + 1);
        let Some(index) = self.link_index_at(at) else { return false };
        self.open_link_occurrence(index, window, cx);
        true
    }

    /// `OpenLinkAction`: the link at each selection's end.
    fn open_links_at_selections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let ends: Vec<(usize, usize)> = editor.selections.iter().map(|selection| {
            let end = selection.end();
            (end.line + 1, utf16_column(&editor.document.lines[end.line], end.column) + 1)
        }).collect();
        for end in ends {
            if let Some(index) = self.link_index_at(end) {
                self.open_link_occurrence(index, window, cx);
            }
        }
    }

    /// `openLinkOccurrence`: the link's url, resolved first when it has none; `file:///./` paths are relative to the
    /// model's file.
    fn open_link_occurrence(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let link = self.links.list[index].clone();
        let supports_resolve = self.language_features.registration(link.handle).is_some_and(|registration| registration.args.get(2) == Some(&json!(true)));
        let resolve = match (link.dto.get("url").filter(|url| !url.is_null()), supports_resolve) {
            (Some(_), _) | (None, false) => None,
            (None, true) => Some(self.host.call("ExtHostLanguageFeatures", "$resolveDocumentLink", vec![json!(link.handle), link.dto["cacheId"].clone()])),
        };
        let model_path = self.editors.active().filter(|editor| editor.document.scheme == "file").map(|editor| editor.document.path.clone());
        cx.spawn_in(window, async move |workbench, cx| {
            let url = match resolve {
                None => link.dto.get("url").cloned().filter(|url| !url.is_null()),
                Some(call) => call.await.ok().and_then(Result::ok).and_then(|resolved| resolved.get("url").cloned()).filter(|url| !url.is_null()),
            };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let Some(url) = url else {
                    workbench.notify(Notification::new(Severity::Warning, "Failed to open this link because its target is missing."));
                    return;
                };
                let target = link_target(&url, model_path.as_deref());
                let options = OpenOptions { allow_commands: true, contributed_openers: ContributedOpeners::Any, ..OpenOptions::default() };
                drop(workbench.open_link(target, options, window, cx));
            });
        })
        .detach();
    }

    /// The link decorations of line `index`: `.detected-link` underlines every link, `.detected-link-active` also
    /// colors the active one. Ranges are byte offsets into the tab-expanded line.
    pub(super) fn link_styles(&self, editor: &OpenEditor, index: usize, tab_size: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        if self.links.model_id != Some(editor.model_id) {
            return Vec::new();
        }
        let text = &editor.document.lines[index];
        let line = index + 1;
        let active_color = self.theme.color("editorLink.activeForeground").map(Hsla::from);
        let expanded = |column: usize| expand_tabs(&text.chars().take(column).collect::<String>(), tab_size).len();
        let mut styles = Vec::new();
        for (link_index, link) in self.links.list.iter().enumerate() {
            if link.start.0 > line || link.end.0 < line {
                continue;
            }
            let start = if link.start.0 == line { char_column(text, link.start.1 - 1) } else { 0 };
            let end = if link.end.0 == line { char_column(text, link.end.1 - 1) } else { text.chars().count() };
            if start >= end {
                continue;
            }
            let active = self.links.active == Some(link_index);
            let style = HighlightStyle {
                underline: Some(UnderlineStyle { thickness: px(1.), color: None, wavy: false }),
                color: if active { active_color } else { None },
                ..HighlightStyle::default()
            };
            styles.push((expanded(start)..expanded(end), style));
        }
        styles
    }

    /// `.detected-link-active { cursor: pointer }`.
    pub(super) fn link_active(&self) -> bool {
        self.links.active.is_some()
    }

    /// The hover message of the links at the anchor (`getHoverMessage`, a decoration's `hoverMessage`).
    pub(super) fn link_hover_parts(&self, editor: &OpenEditor, anchor: Anchor) -> Vec<Part> {
        if self.links.model_id != Some(editor.model_id) {
            return Vec::new();
        }
        let text = &editor.document.lines[anchor.line];
        let column = utf16_column(text, anchor.start) + 1;
        let use_meta_key = !self.multi_cursor_modifier_is_cmd();
        let kb = if use_meta_key { "cmd + click" } else { "option + click" };
        let mut parts = Vec::new();
        for link in &self.links.list {
            let line = anchor.line + 1;
            if !(link.start <= (line, column) && (line, column) <= link.end) {
                continue;
            }
            let start = Position::new(link.start.0 - 1, char_column(&editor.document.lines[link.start.0 - 1], link.start.1 - 1));
            let end = Position::new(link.end.0 - 1, char_column(&editor.document.lines[link.end.0 - 1], link.end.1 - 1));
            let content = hover_message(&link.dto, kb);
            parts.push(Part { participant: Participant::Markdown, ordinal: 0, start, end, content: PartContent::Markdown(vec![content]) });
        }
        parts
    }

    /// `_executeLinkProvider`: the model's links, the first `resolveCount` resolved.
    fn execute_link_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Ok(json!([])));
        };
        let resolve_count = args.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let lines = editor.document.lines.clone();
        let registrations: Vec<(u64, bool)> = self.language_features.ordered(REGISTER, &candidate).iter().rev().map(|registration| (registration.handle, registration.args.get(2) == Some(&json!(true)))).collect();
        let call = self.host.caller();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |_, _| {
            let mut links: Vec<Link> = Vec::new();
            let mut caches = Vec::new();
            for (handle, _) in &registrations {
                let list = match *handle {
                    WORKER_LINK_PROVIDER => worker_links(&lines),
                    _ => match call("ExtHostLanguageFeatures", "$provideDocumentLinks", vec![json!(handle), uri.clone()]).await {
                        Ok(Ok(list)) if list.is_object() => list,
                        _ => continue,
                    },
                };
                if let Some(cache_id) = list["cacheId"].as_u64() {
                    caches.push((*handle, cache_id));
                }
                let new_links = list["links"].as_array().into_iter().flatten().map(|dto| link_from_dto(*handle, dto)).collect();
                links = union(links, new_links);
            }
            let mut result = Vec::new();
            for (index, link) in links.into_iter().enumerate() {
                let mut dto = json!({ "range": link.dto["range"], "url": link.dto["url"], "tooltip": link.dto["tooltip"] });
                let supports_resolve = registrations.iter().any(|(handle, resolves)| *handle == link.handle && *resolves);
                if index < resolve_count && dto["url"].is_null() && supports_resolve
                    && let Ok(Ok(resolved)) = call("ExtHostLanguageFeatures", "$resolveDocumentLink", vec![json!(link.handle), link.dto["cacheId"].clone()]).await
                {
                    dto["url"] = resolved["url"].clone();
                }
                result.push(dto);
            }
            for (handle, cache_id) in caches {
                drop(call("ExtHostLanguageFeatures", "$releaseDocumentLinks", vec![json!(handle), json!(cache_id)]));
            }
            let _ = sender.send(Ok(Value::Array(result)));
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

/// `EditorWorker.computeLinks` as an `ILinksList`.
fn worker_links(lines: &[String]) -> Value {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let links: Vec<Value> = compute_links(&lines)
        .into_iter()
        .map(|link| {
            let text = lines[link.line_number - 1];
            let start = utf16_column(text, link.start_column - 1) + 1;
            let end = utf16_column(text, link.end_column - 1) + 1;
            json!({ "range": { "startLineNumber": link.line_number, "startColumn": start, "endLineNumber": link.line_number, "endColumn": end }, "url": link.url })
        })
        .collect();
    json!({ "links": links })
}

fn link_from_dto(handle: u64, dto: &Value) -> Link {
    let range = &dto["range"];
    let at = |line: &str, column: &str| (range[line].as_u64().unwrap_or(1) as usize, range[column].as_u64().unwrap_or(1) as usize);
    Link { handle, dto: dto.clone(), start: at("startLineNumber", "startColumn"), end: at("endLineNumber", "endColumn") }
}

/// `LinksList._union`: the old links and the new ones that touch none of them, in order of their starts.
fn union(old_links: Vec<Link>, new_links: Vec<Link>) -> Vec<Link> {
    let mut result = Vec::new();
    let (mut old_index, mut new_index) = (0, 0);
    while old_index < old_links.len() && new_index < new_links.len() {
        let (old, new) = (&old_links[old_index], &new_links[new_index]);
        // `Range.areIntersectingOrTouching`.
        if !(old.end < new.start || new.end < old.start) {
            old_index += 1;
            continue;
        }
        if old.start < new.start {
            result.push(old.clone());
            old_index += 1;
        } else {
            result.push(new.clone());
            new_index += 1;
        }
    }
    result.extend_from_slice(&old_links[old_index..]);
    result.extend_from_slice(&new_links[new_index..]);
    result
}

/// `getHoverMessage`: a trusted link to the url with the label, then the gesture; plain text without a url.
fn hover_message(dto: &Value, kb: &str) -> Value {
    let url = url_text(&dto["url"], true);
    let is_command = url.as_deref().is_some_and(|url| url.to_ascii_lowercase().starts_with("command:"));
    let label = match dto["tooltip"].as_str() {
        Some(tooltip) => tooltip.to_owned(),
        None if is_command => "Execute command".to_owned(),
        None => "Follow link".to_owned(),
    };
    let Some(url) = url else {
        return json!({ "value": escape_markdown(&format!("{label} ({kb})")) });
    };
    let title = match is_command {
        true => url["command:".len()..].split(['?', '#']).next().filter(|id| !id.is_empty()).map(|id| format!(" \"{}\"", escape_link_title(&format!("Execute command {id}")))).unwrap_or_default(),
        false => String::new(),
    };
    let destination = escape_link_destination(&url.replace(' ', "%20"));
    json!({ "value": format!("[{}]({destination}{title}) ({kb})", escape_link_text(&label)), "isTrusted": true })
}

/// htmlContent.ts `escapeMarkdownLinkDestination`: blanks and controls percent-encoded, `\()<>` escaped, so the url
/// cannot close the trusted link and start another.
fn escape_link_destination(target: &str) -> String {
    let mut escaped = String::new();
    for character in target.chars() {
        match character {
            c if c.is_whitespace() || c.is_ascii_control() => {
                let mut buffer = [0; 4];
                for byte in c.encode_utf8(&mut buffer).bytes() {
                    escaped.push_str(&format!("%{byte:02X}"));
                }
            }
            '\\' | '(' | ')' | '<' | '>' => {
                escaped.push('\\');
                escaped.push(character);
            }
            c => escaped.push(c),
        }
    }
    escaped
}

/// htmlContent.ts `escapeMarkdownLinkTitle`: line breaks as spaces, NUL replaced, `&\"()` and tabs as character references.
fn escape_link_title(title: &str) -> String {
    let mut escaped = String::new();
    for character in title.replace("\r\n", " ").replace(['\r', '\n'], " ").chars() {
        match character {
            '\0' => escaped.push('\u{FFFD}'),
            '&' | '\\' | '"' | '(' | ')' | '\t' => escaped.push_str(&format!("&#{};", character as u32)),
            c => escaped.push(c),
        }
    }
    escaped
}

/// A link's url: a string as it is, a `UriComponents` written out (`toString(skipEncoding)`).
fn url_text(url: &Value, skip_encoding: bool) -> Option<String> {
    match url {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => crate::uri::Uri::from_json(url).ok().map(|uri| uri.to_string(skip_encoding)),
        _ => None,
    }
}

/// `MarkdownString.appendText` of one line: `escapeMarkdownSyntaxTokens`, blanks as `&nbsp;`, `>` escaped.
fn escape_markdown(text: &str) -> String {
    let mut escaped = String::new();
    let leading_blanks = text.len() - text.trim_start_matches([' ', '\t']).len();
    for (index, character) in text.char_indices() {
        match character {
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '(' | ')' | '#' | '+' | '!' | '~' | '>' => escaped.push('\\'),
            '-' if index == leading_blanks => escaped.push('\\'),
            ' ' | '\t' => {
                escaped.push_str("&nbsp;");
                continue;
            }
            _ => {}
        }
        escaped.push(character);
    }
    escaped
}

/// `escapeMarkdownLinkText`: line breaks as spaces, all ASCII punctuation escaped.
fn escape_link_text(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.replace("\r\n", " ").replace(['\r', '\n'], " ").chars() {
        if character.is_ascii_punctuation() {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// The opener target of a link url; a `file:///./x` url on a file model is relative to the model's file.
fn link_target(url: &Value, model_path: Option<&std::path::Path>) -> Target {
    let text = url_text(url, false).unwrap_or_default();
    if let (Some(model_path), Some(path)) = (model_path, text.strip_prefix("file:///./")) {
        let joined = model_path.join(path);
        return Target::Text(format!("file://{}", joined.display()));
    }
    Target::Text(text)
}

/// The UTF-16 position after `text` inserted at `start`.
pub(super) fn end_after(start: (usize, usize), text: &str) -> (usize, usize) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    match normalized.rsplit_once('\n') {
        None => (start.0, start.1 + normalized.encode_utf16().count()),
        Some((before, last)) => (start.0 + before.matches('\n').count() + 1, last.encode_utf16().count() + 1),
    }
}

/// intervalTree.ts `nodeAcceptEdit` for one position of a `NeverGrowsWhenTypingAtEdges` range: its start sticks
/// to the text after it, its end to the text before it.
pub(super) fn tracked(marker: (usize, usize), start: (usize, usize), end: (usize, usize), new_end: (usize, usize), stick_to_previous: bool) -> (usize, usize) {
    if marker < start || (marker == start && (start != end || stick_to_previous)) {
        return marker;
    }
    if marker <= end {
        return if stick_to_previous { start } else { new_end };
    }
    if marker.0 == end.0 {
        return (new_end.0, new_end.1 + marker.1 - end.1);
    }
    (marker.0 + new_end.0 - end.0, marker.1)
}

#[cfg(test)]
mod tests;
