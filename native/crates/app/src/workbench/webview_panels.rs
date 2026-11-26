//! mainThreadWebviews.ts and mainThreadWebviewPanels.ts: an extension's webview panels as editors of the group,
//! each drawn by a native `Webview` over the editor's body.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use exthost::session::{URL_PROTOCOL, VSBUFFER_KEY};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{Context, Div, Keystroke, Modifiers, Window, canvas, div, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Workbench;
use super::custom_editors::{CustomEditor, CustomEditors};
use super::media_preview::MediaPreview;
use super::webview_views::WEBVIEW_VIEW_SCHEME;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, TextDocument};
use crate::host_sync::WebviewTab;
use crate::theme::ThemeKind;
use crate::webview::{self, Webview, WebviewEvent};

/// `Schemas.webviewPanel`.
pub const WEBVIEW_PANEL_SCHEME: &str = "webview-panel";
/// editorService.ts `ACTIVE_GROUP` and `SIDE_GROUP`, as a `viewColumn` of the show options.
const ACTIVE_GROUP: i64 = -1;
pub(super) const SIDE_GROUP: i64 = -2;
/// webviewEditorInput.ts `WebviewInput.typeId`, the id of a webview editor in the editor state.
pub(super) const WEBVIEW_INPUT: &str = "workbench.editors.webviewInput";
/// mainThreadWebviewPanels.ts `WebviewViewTypeTransformer`: the editor's view type is the extension's with this prefix.
const VIEW_TYPE_PREFIX: &str = "mainThreadWebview-";

#[derive(Default)]
pub struct WebviewPanels {
    pub(super) panels: Vec<WebviewPanel>,
    pub(super) custom_editors: CustomEditors,
    events: Option<UnboundedSender<WebviewEvent>>,
    /// `$registerSerializer` view types.
    serializers: HashSet<String>,
    pub(super) insets: Vec<super::editor_insets::EditorInset>,
    /// The view states the host heard last.
    view_states: Value,
}

pub(super) struct WebviewPanel {
    pub(super) handle: String,
    /// The scheme and path of the editor's model: `webview-panel/webview-<providerId>-<resourceId>`
    /// (webviewEditorInput.ts), or a custom text editor's resource.
    pub(super) scheme: String,
    pub(super) path: PathBuf,
    /// The extension's view type; the editor id is `mainThreadWebview-<viewType>` (`WebviewViewTypeTransformer`).
    pub(super) view_type: String,
    pub(super) title: String,
    pub(super) extension_id: String,
    /// `webviewOriginStore.getOrigin`: one origin per view type and extension, so its storage persists.
    pub(super) origin: String,
    pub(super) html: String,
    /// `IWebviewContentOptions`.
    pub(super) content_options: Value,
    retain_context_when_hidden: bool,
    /// `do-update-state`: what `vscode.setState` stored, given back when a released webview loads again.
    pub(super) state: Value,
    /// `$setIconPath`: the light and the dark theme's icon file.
    icon: Option<(PathBuf, PathBuf)>,
    /// Created when the editor first shows; released while hidden unless `retainContextWhenHidden`.
    pub(super) webview: Option<Webview>,
    /// overlayWebview.ts `_firstLoadPendingMessages`: the messages posted before the first webview was made.
    first_load_messages: Option<Vec<Value>>,
    pub(super) custom: Option<CustomEditor>,
    /// A built-in media preview's page, which talks to the workbench instead of an extension.
    pub(super) media: Option<MediaPreview>,
    /// `enableFindWidget` of the panel options, and the find widget's state (`webviewFindWidgetVisible`,
    /// `webviewFindWidgetFocused`).
    pub(super) find_enabled: bool,
    /// The files of an Images Preview (contrib/imageCarousel), whose page talks to the workbench.
    pub(super) carousel: Option<Vec<PathBuf>>,
    find_visible: bool,
    find_focused: bool,
    /// Restored from the editor state (`LazilyResolvedWebviewEditorInput`): `$deserializeWebviewPanel` waits for
    /// the editor to show and a serializer of its view type.
    revive: bool,
}

impl Workbench {
    pub(crate) fn handle_webview_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let handle = arg(0).as_str().unwrap_or_default().to_owned();
        match method {
            "$createWebviewPanel" => self.create_webview_panel(args, window, cx),
            "$registerSerializer" => {
                self.webview_panels.serializers.insert(handle);
                // A restored panel that shows revives on the next render (webviewWorkbenchService.ts `reviveFor`).
                cx.notify();
            }
            "$unregisterSerializer" => {
                self.webview_panels.serializers.remove(&handle);
            }
            "$disposeWebview" => {
                if let Some((group, index)) = self.webview_editor_group(&handle) {
                    self.with_group(group, |workbench| workbench.editors.close(index));
                    self.close_group_if_empty(group);
                    self.editors_changed(cx);
                }
            }
            "$reveal" => self.reveal_webview_panel(&handle, &arg(1), window, cx),
            "$setTitle" => {
                if let Some(panel) = self.webview_panel_mut(&handle) {
                    panel.title = arg(1).as_str().unwrap_or_default().to_owned();
                    let title = panel.title.clone();
                    if let Some(webview) = &mut panel.webview {
                        webview.send("set-title", json!(title));
                    }
                    self.editors_changed(cx);
                }
            }
            "$setIconPath" => {
                if let Some(panel) = self.webview_panel_mut(&handle) {
                    let file = |uri: &Value| (uri["scheme"] == "file").then(|| PathBuf::from(uri["path"].as_str().unwrap_or_default()));
                    let icon = arg(1);
                    panel.icon = file(&icon["light"]).zip(file(&icon["dark"]));
                    cx.notify();
                }
            }
            "$setHtml" => {
                if let Some(panel) = self.webview_panel_mut(&handle) {
                    panel.html = arg(1).as_str().unwrap_or_default().to_owned();
                    panel.update_content();
                }
            }
            "$setOptions" => {
                if let Some(panel) = self.webview_panel_mut(&handle) {
                    panel.content_options = arg(1);
                    if let Some(webview) = &panel.webview {
                        webview.set_local_resource_roots(local_resource_roots(&panel.content_options));
                    }
                    panel.update_content();
                }
            }
            "$postMessage" => {
                let Some(panel) = self.webview_panel_mut(&handle) else { return Outcome::Ready(Ok(json!(false))) };
                // deserializeWebviewMessage runs in the host page's bridge, where the ArrayBuffers are made.
                let buffers: Vec<Value> = args.iter().skip(2).map(|buffer| buffer[VSBUFFER_KEY].clone()).collect();
                let message = json!({ "message": arg(1), "buffers": buffers });
                match (&mut panel.webview, &mut panel.first_load_messages) {
                    (Some(webview), _) => webview.send("message", message),
                    (None, Some(pending)) => pending.push(message),
                    (None, None) => return Outcome::Ready(Ok(json!(false))),
                }
                return Outcome::Ready(Ok(json!(true)));
            }
            _ => {}
        }
        Outcome::done()
    }

    /// `$createWebviewPanel`: the panel opens pinned in the group of its view column and, unless `preserveFocus`,
    /// takes the focus.
    fn create_webview_panel(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) {
        let (extension, handle, view_type, init, show) = (&args[0], args[1].as_str().unwrap_or_default(), args[2].as_str().unwrap_or_default(), &args[3], &args[4]);
        let path = PathBuf::from(format!("webview-panel/webview-undefined-{handle}"));
        let document = TextDocument::from_text(WEBVIEW_PANEL_SCHEME, &path, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
        let title = init["title"].as_str().unwrap_or_default().to_owned();
        let retain = init["panelOptions"]["retainContextWhenHidden"] == true;
        let mut panel = WebviewPanel::new(handle.to_owned(), WEBVIEW_PANEL_SCHEME.into(), path, view_type, title, extension, init["webviewOptions"].clone(), retain, None);
        panel.find_enabled = init["panelOptions"]["enableFindWidget"] == true;
        self.webview_panels.panels.push(panel);
        let column = &show["viewColumn"];
        let target = self.webview_target_group(column).unwrap_or(self.editors.id);
        let preserve_focus = show["preserveFocus"] == true;
        self.open_in_group(target, preserve_focus, |workbench| workbench.editors.open(document, false));
        self.editors_changed(cx);
        if !preserve_focus {
            self.focus_webview_panel(handle, window, cx);
        }
    }

    /// webviewWorkbenchService.ts `revealWebview`: the panel shows in the group of the view column, moving there
    /// from its group (webviewPanel.contribution.ts `onEditorOpening`); without a column it shows where it is.
    fn reveal_webview_panel(&mut self, handle: &str, show: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some((group, index)) = self.webview_editor_group(handle) else { return };
        let target = self.webview_target_group(&show["viewColumn"]).unwrap_or(group);
        let preserve_focus = show["preserveFocus"] == true;
        self.open_in_group(target, preserve_focus, |workbench| {
            if target == group {
                workbench.editors.activate(index);
                return;
            }
            let editor = workbench.with_group(group, |workbench| workbench.editors.take_editor(index));
            workbench.editors.insert_moved(editor);
        });
        if target != group {
            self.close_group_if_empty(group);
        }
        self.editors_changed(cx);
        if !preserve_focus {
            self.focus_webview_panel(handle, window, cx);
        }
    }

    /// Runs `open` in `group`, which becomes the active group unless `stay`: a preserveFocus open of a webview
    /// leaves the active group (webviewWorkbenchService.ts `EditorActivation.RESTORE`).
    fn open_in_group(&mut self, group: u32, stay: bool, open: impl FnOnce(&mut Self)) {
        match stay {
            true => self.with_group(group, open),
            false => {
                self.activate_group(group);
                open(self);
            }
        }
    }

    /// mainThreadWebviewPanels.ts `getTargetGroupFromShowOptions`, resolved to a group: none for the active group
    /// (no column, ACTIVE, or one empty group); SIDE is the group beside the active one, made when missing; a column
    /// past the last group adds one group after the last.
    pub(super) fn webview_target_group(&mut self, column: &Value) -> Option<u32> {
        let column = column.as_i64().filter(|column| *column != ACTIVE_GROUP)?;
        let groups = self.groups_in_grid_order();
        if groups.len() == 1 && self.editors.editors.is_empty() {
            return None;
        }
        let direction = self.side_by_side_direction();
        if column == SIDE_GROUP {
            return Some(self.group_in_direction(direction).unwrap_or_else(|| self.add_group_beside(direction)));
        }
        let index = usize::try_from(column).ok()?;
        Some(match groups.get(index) {
            Some(&group) => group,
            None => self.with_group(*groups.last().expect("one group at least"), |workbench| workbench.add_group_beside(direction)),
        })
    }

    /// The editor is a webview panel's or a custom editor's, which draws a webview in place of the text.
    pub(super) fn shows_webview(&self, editor: &OpenEditor) -> bool {
        self.webview_panels.panels.iter().any(|panel| panel.shows(editor))
    }

    /// The body of a webview editor: an empty box the native view is placed over once laid out.
    pub(super) fn webview_editor_body(&self, editor: &OpenEditor) -> Div {
        self.webview_body(|panel| panel.shows(editor))
    }

    /// The box the webview of the first matching panel is placed over.
    pub(super) fn webview_body(&self, matches: impl Fn(&WebviewPanel) -> bool) -> Div {
        let view = self.webview_panels.panels.iter().find(|panel| matches(panel)).and_then(|panel| panel.webview.as_ref()).map(Webview::view);
        div().flex_1().min_h_0().child(
            canvas(
                move |bounds, window, _| {
                    if let Some(view) = &view {
                        webview::place(view, window, bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
    }

    /// Runs each render: the active panel's webview exists, the others hide or are released, and a webview
    /// gives the keyboard back once GPUI focused one of its own elements.
    pub(super) fn update_webview_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_webview_views();
        let visible_views = self.visible_webview_views();
        let visible_editors = self.visible_editor_documents();
        // Native views draw over GPUI's layer: a quick input or menu on top of the webview would be hidden.
        // shortcut: the webview hides under these overlays, where Electron draws them over its iframe.
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        // The modal part covers the grid's webviews.
        let modal = self.modal_group_open().then(|| self.editors.group_tabs(crate::editor::MODAL_GROUP)).and_then(|(editors, active)| active.map(|index| editors[index].document()));
        let modal = modal.map(|document| (document.scheme.clone(), document.path.clone()));
        let gpui_focused = window.focused(cx).is_some();
        let mut created = Vec::new();
        let mut revived = Vec::new();
        for panel in &mut self.webview_panels.panels {
            let shown = match panel.scheme == WEBVIEW_VIEW_SCHEME {
                true => visible_views.contains(&panel.path),
                false => visible_editors.iter().any(|(scheme, path)| panel.scheme == *scheme && panel.path == *path),
            };
            // WKWebView needs the main thread, which cargo test threads are not.
            if shown && panel.webview.is_none() && !cfg!(test) {
                created.push(panel.handle.clone());
            }
            if shown && panel.revive {
                revived.push(panel.handle.clone());
            }
            let under_modal = modal.as_ref().is_some_and(|(scheme, path)| !(panel.scheme == *scheme && panel.path == *path));
            match (&panel.webview, shown) {
                (Some(webview), true) if covered || under_modal => webview.hide(),
                (Some(webview), true) if gpui_focused => webview.release_keyboard(),
                (Some(webview), false) if panel.retain_context_when_hidden => webview.hide(),
                (Some(_), false) => panel.webview = None,
                _ => {}
            }
        }
        for handle in created {
            self.create_webview(&handle, window, cx);
        }
        for handle in revived {
            self.revive_webview_panel(&handle);
        }
        self.update_editor_insets(window, cx);
    }

    /// mainThreadWebviewPanels.ts revivers of a restored panel that shows: without a serializer of its view type the
    /// extension activates (`onWebviewPanel:<viewType>`); with one, the host fills the panel in `$deserializeWebviewPanel`.
    fn revive_webview_panel(&mut self, handle: &str) {
        let Some((group, _)) = self.webview_editor_group(handle) else { return };
        let position = self.groups_in_grid_order().iter().position(|id| *id == group);
        let index = self.webview_panels.panels.iter().position(|panel| panel.handle == handle).expect("a shown panel");
        let active = self.editors.active().is_some_and(|editor| self.webview_panels.panels[index].shows(editor));
        let panel = &mut self.webview_panels.panels[index];
        if !self.webview_panels.serializers.contains(&panel.view_type) {
            let event = format!("onWebviewPanel:{}", panel.view_type);
            drop(self.host.activate_by_event(&event));
            return;
        }
        panel.revive = false;
        let state = panel.state.as_str().and_then(|state| serde_json::from_str(state).ok()).unwrap_or(Value::Null);
        let init = json!({
            "title": panel.title,
            "state": state,
            "panelOptions": { "enableFindWidget": panel.find_enabled, "retainContextWhenHidden": panel.retain_context_when_hidden },
            "webviewOptions": panel.content_options,
            "active": active,
        });
        let args = vec![json!(handle), json!(panel.view_type), init, json!(position)];
        self.host.send("ExtHostWebviewPanels", "$deserializeWebviewPanel", args);
    }

    /// webviewEditorInputSerializer.ts `toJson` of a panel `shouldPersist` keeps: one restored, or one whose view type
    /// has a serializer.
    pub(super) fn serialized_webview_editor(&self, editor: &OpenEditor) -> Option<Value> {
        let panel = self.webview_panels.panels.iter().find(|panel| panel.scheme == WEBVIEW_PANEL_SCHEME && panel.is_extensions() && panel.custom.is_none() && panel.shows(editor))?;
        if !panel.revive && !self.webview_panels.serializers.contains(&panel.view_type) {
            return None;
        }
        let mut options = panel.content_options.clone();
        options["enableFindWidget"] = json!(panel.find_enabled);
        options["retainContextWhenHidden"] = json!(panel.retain_context_when_hidden);
        let icon = panel.icon.as_ref().map(|(light, dark)| json!({ "light": exthost::scan::file_uri(light), "dark": exthost::scan::file_uri(dark) }));
        Some(json!({
            "origin": panel.origin,
            "viewType": format!("{VIEW_TYPE_PREFIX}{}", panel.view_type),
            "providedId": panel.view_type,
            "title": panel.title,
            "options": options,
            "extensionId": panel.extension_id,
            "state": panel.state,
            "iconPath": icon,
        }))
    }

    /// webviewEditorInputSerializer.ts `deserialize` and `openRevivedWebview`: the panel's editor, filled in once it shows.
    pub(super) fn restore_webview_editor(&mut self, value: &Value) -> Option<TextDocument> {
        let view_type = value["viewType"].as_str()?.strip_prefix(VIEW_TYPE_PREFIX)?;
        let handle = crate::debug::generate_uuid();
        let path = PathBuf::from(format!("webview-panel/webview-undefined-{handle}"));
        let document = TextDocument::from_text(WEBVIEW_PANEL_SCHEME, &path, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
        let options = &value["options"];
        let title = value["title"].as_str().unwrap_or_default().to_owned();
        let extension = json!({ "id": value["extensionId"] });
        let mut panel = WebviewPanel::new(handle, WEBVIEW_PANEL_SCHEME.into(), path, view_type, title, &extension, options.clone(), options["retainContextWhenHidden"] == true, None);
        panel.find_enabled = options["enableFindWidget"] == true;
        panel.state = value["state"].clone();
        let file = |uri: &Value| (uri["scheme"] == "file").then(|| PathBuf::from(uri["path"].as_str().unwrap_or_default()));
        panel.icon = file(&value["iconPath"]["light"]).zip(file(&value["iconPath"]["dark"]));
        panel.revive = true;
        self.webview_panels.panels.push(panel);
        Some(document)
    }

    /// The scheme and path of each group's active editor, the modal group's included while it is open.
    fn visible_editor_documents(&self) -> Vec<(String, PathBuf)> {
        let modal = self.modal_group_open().then_some(crate::editor::MODAL_GROUP);
        let groups = self.groups_in_grid_order().into_iter().chain(modal);
        let active = groups.filter_map(|group| {
            let (editors, active) = self.editors.group_tabs(group);
            active.map(|index| (editors[index].document().scheme.clone(), editors[index].document().path.clone()))
        });
        active.collect()
    }

    /// webviewEditor.ts `setEditorVisible(false)` and `clearInput`: a panel's webview without `retainContextWhenHidden` is released
    /// as its editor hides, not at the next frame, so a reveal right after loads it again.
    fn release_hidden_webview_panels(&mut self) {
        let visible_editors = self.visible_editor_documents();
        for panel in &mut self.webview_panels.panels {
            let shown = visible_editors.iter().any(|(scheme, path)| panel.scheme == *scheme && panel.path == *path);
            if panel.scheme != WEBVIEW_VIEW_SCHEME && !shown && !panel.retain_context_when_hidden {
                panel.webview = None;
            }
        }
    }

    fn create_webview(&mut self, handle: &str, window: &mut Window, cx: &mut Context<Self>) {
        // gpui tests run off the main thread, where AppKit views cannot be made.
        if cfg!(test) {
            return;
        }
        let events = self.webview_events(window, cx);
        let pre = self.app_root.join("src/vs/workbench/contrib/webview/browser/pre");
        let styles = self.webview_styles();
        let panel = self.webview_panel_mut(handle).expect("a shown panel");
        let mut webview = Webview::new(window, &panel.handle, &panel.origin, &panel.extension_id, pre, local_resource_roots(&panel.content_options), events);
        webview.send("styles", styles);
        panel.webview = Some(webview);
        panel.update_content();
        let webview = panel.webview.as_mut().expect("just made");
        for message in panel.first_load_messages.take().unwrap_or_default() {
            webview.send("message", message);
        }
    }

    /// The channel the webviews' host pages post on, read on the window's executor.
    pub(super) fn webview_events(&mut self, window: &mut Window, cx: &mut Context<Self>) -> UnboundedSender<WebviewEvent> {
        if let Some(events) = &self.webview_panels.events {
            return events.clone();
        }
        let (sender, mut receiver) = unbounded();
        cx.spawn_in(window, async move |workbench, cx| {
            while let Some(event) = receiver.next().await {
                if workbench.update_in(cx, |workbench, window, cx| workbench.handle_webview_event(event, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        self.webview_panels.events = Some(sender.clone());
        sender
    }

    /// webviewElement.ts `on(...)` handlers, with mainThreadWebviews.ts forwarding to the extension host.
    fn handle_webview_event(&mut self, event: WebviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.id.starts_with("extension-editor-") {
            return self.handle_extension_editor_event(&event.id, &event.channel, &event.data, window, cx);
        }
        if event.id.starts_with(super::editor_insets::INSET_ID_PREFIX) {
            return self.handle_editor_inset_event(event);
        }
        if let Some(tab) = event.id.strip_prefix(super::notebook_editor::WEBVIEW_ID_PREFIX) {
            return self.handle_notebook_webview_event(tab.parse().unwrap_or_default(), &event.channel, &event.data, window, cx);
        }
        let Some(panel) = self.webview_panel_mut(&event.id) else { return };
        let data = event.data;
        match event.channel.as_str() {
            "webview-ready" => {
                if let Some(webview) = &mut panel.webview {
                    webview.did_become_ready();
                }
            }
            "onmessage" if panel.carousel.is_some() => {
                let message = data["message"].as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
                self.handle_carousel_message(&event.id, &message, cx);
            }
            "onmessage" if panel.media.is_some() => {
                let message = data["message"].as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
                self.handle_media_preview_message(&event.id, &message, window, cx);
            }
            "onmessage" => {
                // serializeWebviewMessage ran in the host page's bridge: JSON text, the ArrayBuffers as base64.
                self.host.send("ExtHostWebviews", "$onMessage", vec![json!(event.id), data["message"].clone(), data["buffers"].clone()]);
            }
            "do-update-state" => {
                panel.state = data.clone();
                if panel.scheme == WEBVIEW_VIEW_SCHEME {
                    let view_id = panel.view_type.clone();
                    self.save_webview_view_state(&view_id, &data);
                }
            }
            "no-csp-found" => {
                let extension_id = panel.extension_id.clone();
                self.host.send("ExtHostWebviews", "$onMissingCsp", vec![json!(event.id), json!(extension_id)]);
            }
            "did-click-link" => {
                let enable_command_uris = panel.content_options["enableCommandUris"].clone();
                self.open_webview_link(data["uri"].as_str().unwrap_or_default(), &enable_command_uris, window, cx);
            }
            "did-focus" => {
                // The webview has the keyboard; GPUI's elements lose the focus, as the iframe's focus blurs them.
                if let Some(webview) = &panel.webview {
                    webview.take_keyboard();
                }
                window.blur(cx);
            }
            "did-keydown" => {
                if let Some(keystroke) = keystroke(&data) {
                    window.dispatch_keystroke(keystroke, cx);
                }
            }
            "find-focus" => {
                panel.find_focused = data["focused"] == true;
                if panel.find_focused {
                    window.blur(cx);
                }
            }
            "find-hidden" => {
                (panel.find_visible, panel.find_focused) = (false, false);
                // `WebviewFindWidget.hide` gives the focus back to the webview.
                if let Some(webview) = &mut panel.webview {
                    webview.focus();
                }
            }
            "fatal-error" => eprintln!("[webview] {}: {}", event.id, data["message"].as_str().unwrap_or_default()),
            _ => {}
        }
    }

    /// mainThreadWebviews.ts `onDidClickLink`: `isSupportedLink`, then the opener service.
    pub(super) fn open_webview_link(&mut self, link: &str, enable_command_uris: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (scheme, rest) = link.split_once(':').unwrap_or_default();
        match scheme {
            "http" | "https" | "mailto" => cx.open_url(link),
            URL_PROTOCOL => self.open_app_uri(link, window, cx),
            "command" => {
                let command = rest.split_once('?').map_or(rest, |(command, _)| command);
                let allowed = enable_command_uris == true || enable_command_uris.as_array().is_some_and(|ids| ids.iter().any(|id| id == command));
                if allowed {
                    let args = rest.split_once('?').and_then(|(_, query)| serde_json::from_str::<Value>(&percent_decode(query)).ok());
                    let args = match args {
                        Some(Value::Array(args)) => args,
                        Some(arg) => vec![arg],
                        None => Vec::new(),
                    };
                    drop(self.execute_command(command, args, window, cx));
                }
            }
            _ => {}
        }
    }

    /// Focuses the panel's webview, or does once it is created on the next render.
    pub(super) fn focus_webview_panel(&mut self, handle: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_webview_panels(window, cx);
        window.blur(cx);
        if let Some(webview) = self.webview_panel_mut(handle).and_then(|panel| panel.webview.as_mut()) {
            webview.focus();
        }
    }

    /// Run from `editors_changed`: panels whose editor closed are disposed, tabs get titles, and the host hears
    /// where each panel shows (`updateWebviewViewStates`).
    pub(super) fn sync_webview_panels(&mut self) {
        let editors = &self.editors;
        let group_editors = editors.editors.iter().chain(editors.other_groups.iter().flat_map(|group| &group.editors));
        let (open, closed): (Vec<WebviewPanel>, Vec<WebviewPanel>) =
            std::mem::take(&mut self.webview_panels.panels).into_iter().partition(|panel| panel.scheme == WEBVIEW_VIEW_SCHEME || group_editors.clone().any(|editor| panel.shows(editor)));
        self.webview_panels.panels = open;
        self.release_hidden_webview_panels();
        for panel in closed.into_iter().filter(|panel| panel.is_extensions()) {
            self.host.send("ExtHostWebviewPanels", "$onDidDisposeWebviewPanel", vec![json!(panel.handle)]);
            // The custom document's last editor closed: its model reference is released.
            let Some(custom) = panel.custom.filter(|custom| custom.resolved && panel.scheme == WEBVIEW_PANEL_SCHEME) else { continue };
            let open = self.webview_panels.panels.iter().any(|other| other.view_type == panel.view_type && other.custom.as_ref().is_some_and(|other| other.resource == custom.resource));
            if !open {
                self.custom_document_disposed(&custom.resource, &panel.view_type);
                self.host.send("ExtHostCustomEditors", "$disposeCustomDocument", vec![custom.resource, json!(panel.view_type)]);
            }
        }
        self.host.webview_tabs = self
            .webview_panels
            .panels
            .iter()
            .filter(|panel| panel.scheme != WEBVIEW_VIEW_SCHEME)
            .map(|panel| {
                let tab = match &panel.custom {
                    Some(custom) => WebviewTab {
                        label: panel.title.clone(),
                        view_type: panel.view_type.clone(),
                        custom_resource: Some(custom.resource.clone()),
                        custom_text: panel.scheme != WEBVIEW_PANEL_SCHEME,
                    },
                    // An editor that is no webview input upstream: TabInputKind.UnknownInput.
                    None if panel.carousel.is_some() => WebviewTab { label: panel.title.clone(), view_type: panel.view_type.clone(), custom_resource: None, custom_text: false },
                    None => WebviewTab { label: panel.title.clone(), view_type: format!("mainThreadWebview-{}", panel.view_type), custom_resource: None, custom_text: false },
                };
                (panel.path.clone(), tab)
            })
            .collect();
        let states = Value::Object(self.webview_panel_view_states());
        if states.as_object().is_some_and(serde_json::Map::is_empty) || states == self.webview_panels.view_states {
            return;
        }
        self.webview_panels.view_states = states.clone();
        self.host.send("ExtHostWebviewPanels", "$onDidChangeWebviewPanelViewStates", vec![states]);
    }

    /// Each editor panel's state: visible as its group's active editor, active as the active editor, and its
    /// group's index in grid order (`editorGroupToColumn`, which the host turns into a ViewColumn).
    fn webview_panel_view_states(&self) -> serde_json::Map<String, Value> {
        let active_group = self.editors.id;
        let mut states = serde_json::Map::new();
        for (column, group) in self.groups_in_grid_order().into_iter().enumerate() {
            let (editors, active_index) = self.editors.group_tabs(group);
            for (index, editor) in editors.iter().enumerate() {
                let Some(panel) = self.webview_panels.panels.iter().find(|panel| panel.scheme != WEBVIEW_VIEW_SCHEME && panel.is_extensions() && panel.shows(editor)) else { continue };
                let visible = active_index == Some(index);
                states.insert(panel.handle.clone(), json!({ "visible": visible, "active": visible && group == active_group, "position": column }));
            }
        }
        states
    }

    /// webviewCommands.ts (find in the active webview editor) and the Developer commands of webview.contribution.ts.
    pub(super) fn register_webview_actions(&mut self) {
        self.commands.register("editor.action.webvieweditor.showFind", |workbench, _, _, _| {
            let styles = workbench.webview_styles();
            let icon = |name: &str| std::fs::read_to_string(crate::codicons::svg_path(&workbench.app_root, name)).unwrap_or_default();
            let icons = [icon("arrow-up"), icon("arrow-down"), icon("close")];
            if let Some(panel) = workbench.active_webview_panel_mut().filter(|panel| panel.find_enabled) {
                panel.find_visible = true;
                if let Some(webview) = &panel.webview {
                    webview.show_find(&styles, icons);
                }
            }
            Outcome::done()
        });
        self.commands.register("editor.action.webvieweditor.hideFind", |workbench, _, _, _| {
            if let Some(webview) = workbench.active_webview_panel_mut().and_then(|panel| panel.webview.as_ref()) {
                webview.hide_find();
            }
            Outcome::done()
        });
        for (command, previous) in [("editor.action.webvieweditor.findNext", false), ("editor.action.webvieweditor.findPrevious", true)] {
            self.commands.register(command, move |workbench, _, _, _| {
                if let Some(webview) = workbench.active_webview_panel_mut().and_then(|panel| panel.webview.as_ref()) {
                    webview.find(previous);
                }
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.webview.openDeveloperTools", |workbench, _, _, _| {
            for webview in workbench.webview_panels.panels.iter().filter_map(|panel| panel.webview.as_ref()) {
                webview.open_inspector();
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.webview.reloadWebviewAction", |workbench, _, _, _| {
            for panel in &mut workbench.webview_panels.panels {
                panel.update_content();
            }
            Outcome::done()
        });
    }

    /// The panel of the active editor when it is a webview editor (`WebviewEditor`), not a built-in media preview.
    fn active_webview_panel_mut(&mut self) -> Option<&mut WebviewPanel> {
        let active = self.editors.active()?;
        let index = self.webview_panels.panels.iter().position(|panel| panel.is_extensions() && panel.shows(active))?;
        Some(&mut self.webview_panels.panels[index])
    }

    /// The context keys of the active webview editor: its editor id and its find widget's state.
    pub(super) fn webview_editor_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(active) = self.editors.active() else { return };
        let Some(panel) = self.webview_panels.panels.iter().find(|panel| panel.is_extensions() && panel.shows(active)) else { return };
        context.insert("activeEditor".into(), json!("WebviewEditor"));
        // webviewWorkbenchService.ts `getWebviewId` and customEditors.ts `getActiveCustomEditorId`, per group.
        context.insert("activeWebviewPanelId".into(), json!(panel.view_type));
        if panel.custom.is_some() {
            context.insert("activeCustomEditorId".into(), json!(panel.view_type));
        }
        context.insert("webviewFindWidgetEnabled".into(), json!(panel.find_enabled));
        context.insert("webviewFindWidgetVisible".into(), json!(panel.find_visible));
        context.insert("webviewFindWidgetFocused".into(), json!(panel.find_focused));
    }

    /// Sends the theme to every webview, as `WebviewThemeDataProvider.onThemeDataChanged` does.
    pub(super) fn restyle_webviews(&mut self) {
        let styles = self.webview_styles();
        for webview in self.webview_panels.panels.iter_mut().filter_map(|panel| panel.webview.as_mut()) {
            webview.send("styles", styles.clone());
        }
        for webview in self.webview_panels.insets.iter_mut().filter_map(|inset| inset.webview.as_mut()) {
            webview.send("styles", styles.clone());
        }
    }

    /// The title of a webview editor's tab, `WebviewInput.getName`.
    pub(super) fn webview_title(&self, path: &Path) -> Option<String> {
        self.webview_panel(path).map(|panel| panel.title.clone())
    }

    pub(super) fn webview_styles(&self) -> Value {
        let label = self.color_themes.iter().find(|entry| entry.settings_id == self.theme.settings_id).map_or(self.theme.settings_id.clone(), |entry| entry.label.clone());
        webview::theme_data(&self.theme, &label, &self.setting("editor"), self.setting("accessibility.underlineLinks") == true, self.is_motion_reduced())
    }

    /// webviewIconManager.ts: the dark icon for dark and high contrast themes, the light one otherwise.
    pub(super) fn webview_icon(&self, path: &Path) -> Option<&Path> {
        let (light, dark) = self.webview_panel(path)?.icon.as_ref()?;
        Some(match self.theme.kind {
            ThemeKind::Dark | ThemeKind::HighContrastDark => dark,
            ThemeKind::Light | ThemeKind::HighContrastLight => light,
        })
    }

    /// The file a custom editor or media preview panel shows.
    pub(super) fn webview_resource(&self, path: &Path) -> Option<PathBuf> {
        let panel = self.webview_panel(path)?;
        if let Some(media) = &panel.media {
            return Some(media.path.clone());
        }
        let resource = &panel.custom.as_ref()?.resource;
        (resource["scheme"] == "file").then(|| PathBuf::from(resource["path"].as_str().unwrap_or_default()))
    }

    fn webview_panel(&self, path: &Path) -> Option<&WebviewPanel> {
        self.webview_panels.panels.iter().find(|panel| panel.path == path)
    }

    pub(super) fn webview_panel_mut(&mut self, handle: &str) -> Option<&mut WebviewPanel> {
        self.webview_panels.panels.iter_mut().find(|panel| panel.handle == handle)
    }

    /// The group and index of the editor that shows the panel, in any group of the grid.
    pub(super) fn webview_editor_group(&self, handle: &str) -> Option<(u32, usize)> {
        let panel = self.webview_panels.panels.iter().find(|panel| panel.handle == handle)?;
        self.groups_in_grid_order().into_iter().find_map(|group| self.editors.group_tabs(group).0.iter().position(|editor| panel.shows(editor)).map(|index| (group, index)))
    }
}

impl WebviewPanel {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(handle: String, scheme: String, path: PathBuf, view_type: &str, title: String, extension: &Value, content_options: Value, retain_context_when_hidden: bool, custom: Option<CustomEditor>) -> Self {
        let extension_id = extension["id"]["value"].as_str().or(extension["id"].as_str()).unwrap_or_default().to_owned();
        WebviewPanel {
            handle,
            scheme,
            path,
            view_type: view_type.to_owned(),
            title,
            origin: webview_origin(view_type, &extension_id),
            extension_id,
            html: String::new(),
            content_options,
            retain_context_when_hidden,
            state: Value::Null,
            icon: None,
            webview: None,
            first_load_messages: Some(Vec::new()),
            custom,
            media: None,
            find_enabled: false,
            carousel: None,
            find_visible: false,
            find_focused: false,
            revive: false,
        }
    }

    /// A panel an extension made, not a built-in page (media preview, Images Preview).
    fn is_extensions(&self) -> bool {
        self.media.is_none() && self.carousel.is_none()
    }

    pub(super) fn shows(&self, editor: &OpenEditor) -> bool {
        editor.document().is(&self.scheme, &self.path)
    }

    /// webviewElement.ts `doUpdateContent`.
    pub(super) fn update_content(&mut self) {
        let Some(webview) = &mut self.webview else { return };
        let allow_scripts = self.content_options["enableScripts"] == true;
        let allow_forms = self.content_options["enableForms"].as_bool().unwrap_or(allow_scripts);
        webview.send(
            "content",
            json!({
                "contents": self.html,
                "title": self.title,
                "options": { "allowMultipleAPIAcquire": false, "allowScripts": allow_scripts, "allowForms": allow_forms },
                "state": self.state,
                "cspSource": "'self' https://*.vscode-cdn.net",
                "confirmBeforeClose": "keyboardOnly",
            }),
        );
    }
}

/// The `file` paths of `localResourceRoots`; the extension host already put in the defaults.
pub(super) fn local_resource_roots(options: &Value) -> Vec<PathBuf> {
    let roots = options["localResourceRoots"].as_array().into_iter().flatten();
    roots.filter(|root| root["scheme"] == "file").filter_map(|root| root["path"].as_str()).map(PathBuf::from).collect()
}

/// A UUID-shaped origin that stays the same for a view type of an extension, as the stored one upstream does.
fn webview_origin(view_type: &str, extension_id: &str) -> String {
    let digest = Sha256::digest(format!("{extension_id}/{view_type}").as_bytes());
    let hex: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

pub(super) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes.get(index + 1..index + 3).and_then(|hex| std::str::from_utf8(hex).ok()).and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A `did-keydown` from the host page as the keystroke GPUI would have seen: index.html forwards the keys its
/// frame does not handle, which upstream go to the workbench's keybinding service.
pub(super) fn keystroke(data: &Value) -> Option<Keystroke> {
    let code = data["code"].as_str().unwrap_or_default();
    let key = data["key"].as_str()?;
    let key = if let Some(letter) = code.strip_prefix("Key") {
        letter.to_ascii_lowercase()
    } else if let Some(digit) = code.strip_prefix("Digit") {
        digit.to_owned()
    } else {
        match key {
            "ArrowUp" => "up".into(),
            "ArrowDown" => "down".into(),
            "ArrowLeft" => "left".into(),
            "ArrowRight" => "right".into(),
            "PageUp" => "pageup".into(),
            "PageDown" => "pagedown".into(),
            " " => "space".into(),
            "Meta" | "Shift" | "Control" | "Alt" => return None,
            key if key.chars().count() == 1 => key.to_lowercase(),
            key => key.to_ascii_lowercase(),
        }
    };
    let modifiers = Modifiers { control: data["ctrlKey"] == true, alt: data["altKey"] == true, shift: data["shiftKey"] == true, platform: data["metaKey"] == true, function: false };
    Some(Keystroke { modifiers, key, key_char: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_forwarded_key_events_to_keystrokes() {
        let event = |key: &str, code: &str, meta: bool, shift: bool| json!({ "key": key, "code": code, "metaKey": meta, "shiftKey": shift });
        let parsed = |data: Value| keystroke(&data).map(|keystroke| keystroke.unparse());
        assert_eq!(parsed(event("P", "KeyP", true, true)), Some("cmd-shift-p".into()));
        assert_eq!(parsed(event("!", "Digit1", false, true)), Some("shift-1".into()));
        assert_eq!(parsed(event("F1", "F1", false, false)), Some("f1".into()));
        assert_eq!(parsed(event("ArrowUp", "ArrowUp", false, false)), Some("up".into()));
        assert_eq!(parsed(event("Escape", "Escape", false, false)), Some("escape".into()));
        assert_eq!(parsed(event("Shift", "ShiftLeft", false, true)), None);
    }

    #[gpui::test]
    fn panels_open_in_their_view_column_and_move_on_reveal(cx: &mut gpui::TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-webview-columns-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "# a").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        let file = json!({ "scheme": "file", "path": dir.join("a.md").to_string_lossy() });
        workbench.update_in(cx, |workbench, window, cx| workbench.api_open_editor(&[file], window, cx)).unwrap();
        let request = |method: &str, args: Vec<Value>, cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_webview_request(method, &args, window, cx)));
        let states = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| (workbench.groups_in_grid_order().len(), Value::Object(workbench.webview_panel_view_states())));
        let init = json!({ "title": "Preview", "panelOptions": {}, "webviewOptions": {} });

        // Beside with preserveFocus adds a group and leaves the source's group active.
        request("$createWebviewPanel", vec![json!({ "id": "ext" }), json!("h1"), json!("preview"), init, json!({ "viewColumn": SIDE_GROUP, "preserveFocus": true })], cx);
        assert_eq!(states(cx), (2, json!({ "h1": { "visible": true, "active": false, "position": 1 } })));

        // A message posted before the first webview is made waits for it.
        let posted = workbench.update_in(cx, |workbench, window, cx| workbench.handle_webview_request("$postMessage", &[json!("h1"), json!("{}")], window, cx));
        assert!(matches!(posted, Outcome::Ready(Ok(Value::Bool(true)))));
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.webview_panels.panels[0].first_load_messages.as_ref().map(Vec::len)), Some(1));

        // A reveal in column One moves the panel there; its emptied group closes.
        request("$reveal", vec![json!("h1"), json!({ "viewColumn": 0, "preserveFocus": true })], cx);
        assert_eq!(states(cx), (1, json!({ "h1": { "visible": true, "active": true, "position": 0 } })));
        let mut context = std::collections::HashMap::new();
        workbench.read_with(cx, |workbench, _| workbench.webview_editor_context(&mut context));
        assert_eq!(context.get("activeWebviewPanelId"), Some(&json!("preview")));

        request("$disposeWebview", vec![json!("h1")], cx);
        assert!(workbench.read_with(cx, |workbench, _| workbench.webview_panels.panels.is_empty()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn panels_with_a_serializer_are_restored_from_the_editor_state(cx: &mut gpui::TestAppContext) {
        const EDITOR_PART_KEY: &str = "memento/workbench.parts.editor";
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let (sender, host) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(sender), cx));
        let request = |method: &str, args: Vec<Value>, cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_webview_request(method, &args, window, cx)));
        let sent = |method: &str| host.try_iter().filter(|message| message.method == method).map(|message| message.args).collect::<Vec<_>>();
        let init = json!({ "title": "Preview", "panelOptions": { "enableFindWidget": true }, "webviewOptions": { "enableScripts": true } });
        request("$createWebviewPanel", vec![json!({ "id": "ext" }), json!("h1"), json!("preview"), init.clone(), json!({})], cx);
        workbench.update(cx, |workbench, _| workbench.webview_panels.panels[0].state = json!("{\"page\":2}"));

        // Without a serializer the panel is not kept.
        let saved = workbench.update(cx, |workbench, _| {
            workbench.save_editor_part_state();
            workbench.workspace_storage.lock().unwrap().get(EDITOR_PART_KEY).unwrap().to_owned()
        });
        request("$disposeWebview", vec![json!("h1")], cx);
        workbench.update(cx, |workbench, _| {
            workbench.workspace_storage.lock().unwrap().set(EDITOR_PART_KEY, saved);
            workbench.restore_editor_part_state();
        });
        assert!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.is_empty()));

        request("$createWebviewPanel", vec![json!({ "id": "ext" }), json!("h2"), json!("preview"), init, json!({})], cx);
        workbench.update(cx, |workbench, _| workbench.webview_panels.panels[0].state = json!("{\"page\":2}"));
        request("$registerSerializer", vec![json!("preview")], cx);
        // The state a window closing with the panel leaves, read by the next window on that workspace.
        let saved = workbench.update(cx, |workbench, _| {
            workbench.save_editor_part_state();
            workbench.workspace_storage.lock().unwrap().get(EDITOR_PART_KEY).unwrap().to_owned()
        });
        request("$disposeWebview", vec![json!("h2")], cx);
        request("$unregisterSerializer", vec![json!("preview")], cx);
        workbench.update(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().set(EDITOR_PART_KEY, saved));
        drop(sent(""));

        // The restored editor shows: its extension activates, and once the serializer registers the host fills it in.
        workbench.update_in(cx, |workbench, window, cx| {
            workbench.restore_editor_part_state();
            workbench.update_webview_panels(window, cx);
        });
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.webview_title(&workbench.editors.active().unwrap().document().path)), Some("Preview".into()));
        assert!(sent("$activateByEvent").iter().any(|args| args[0] == "onWebviewPanel:preview"));
        request("$registerSerializer", vec![json!("preview")], cx);
        workbench.update_in(cx, |workbench, window, cx| workbench.update_webview_panels(window, cx));
        let deserialize = sent("$deserializeWebviewPanel");
        assert_eq!(deserialize.len(), 1);
        let handle = workbench.read_with(cx, |workbench, _| workbench.webview_panels.panels[0].handle.clone());
        assert_eq!(deserialize[0], vec![json!(handle), json!("preview"), json!({ "title": "Preview", "state": { "page": 2 }, "panelOptions": { "enableFindWidget": true, "retainContextWhenHidden": false }, "webviewOptions": { "enableScripts": true, "enableFindWidget": true, "retainContextWhenHidden": false }, "active": true }), json!(0)]);
    }

    #[test]
    fn reads_file_roots_and_stable_origins() {
        let options = json!({ "localResourceRoots": [{ "scheme": "file", "path": "/a" }, { "scheme": "vscode-vfs", "path": "/b" }] });
        assert_eq!(local_resource_roots(&options), vec![PathBuf::from("/a")]);
        assert_eq!(webview_origin("t", "e"), webview_origin("t", "e"));
        assert_ne!(webview_origin("t", "e"), webview_origin("u", "e"));
        assert_eq!(webview_origin("t", "e").len(), 36);
        assert_eq!(percent_decode("%5B%22a%22%5D"), "[\"a\"]");
    }
}
