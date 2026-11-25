//! mainThreadWebviews.ts and mainThreadWebviewPanels.ts: an extension's webview panels as editors of the group,
//! each drawn by a native `Webview` over the editor's body.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{Context, Div, Keystroke, Modifiers, Window, canvas, div, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, TextDocument};
use crate::host_sync::WebviewTab;
use crate::theme::ThemeKind;
use crate::webview::{self, Webview, WebviewEvent};

/// `Schemas.webviewPanel`.
pub const WEBVIEW_PANEL_SCHEME: &str = "webview-panel";

#[derive(Default)]
pub struct WebviewPanels {
    panels: Vec<WebviewPanel>,
    events: Option<UnboundedSender<WebviewEvent>>,
    /// `$registerSerializer` view types.
    serializers: HashSet<String>,
    /// The handle whose view state the host heard last as active (`onDidChangeActiveWebviewEditor`).
    active: Option<String>,
}

struct WebviewPanel {
    handle: String,
    /// The editor's model path: `webview-panel/webview-<providerId>-<resourceId>` (webviewEditorInput.ts).
    path: PathBuf,
    /// The extension's view type; the editor id is `mainThreadWebview-<viewType>` (`WebviewViewTypeTransformer`).
    view_type: String,
    title: String,
    extension_id: String,
    /// `webviewOriginStore.getOrigin`: one origin per view type and extension, so its storage persists.
    origin: String,
    html: String,
    /// `IWebviewContentOptions`.
    content_options: Value,
    retain_context_when_hidden: bool,
    /// `do-update-state`: what `vscode.setState` stored, given back when a released webview loads again.
    state: Value,
    /// `$setIconPath`: the light and the dark theme's icon file.
    icon: Option<(PathBuf, PathBuf)>,
    /// Created when the editor first shows; released while hidden unless `retainContextWhenHidden`.
    webview: Option<Webview>,
}

impl Workbench {
    pub(crate) fn handle_webview_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let handle = arg(0).as_str().unwrap_or_default().to_owned();
        match method {
            "$createWebviewPanel" => self.create_webview_panel(args, window, cx),
            "$registerSerializer" => {
                self.webview_panels.serializers.insert(handle);
            }
            "$unregisterSerializer" => {
                self.webview_panels.serializers.remove(&handle);
            }
            "$disposeWebview" => {
                if let Some(index) = self.webview_editor_index(&handle) {
                    self.editors.close(index);
                    self.editors_changed(cx);
                }
            }
            "$reveal" => {
                if let Some(index) = self.webview_editor_index(&handle) {
                    self.editors.activate(index);
                    self.editors_changed(cx);
                    if arg(1)["preserveFocus"] != true {
                        self.focus_webview_panel(&handle, window, cx);
                    }
                }
            }
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
                let Some(webview) = self.webview_panel_mut(&handle).and_then(|panel| panel.webview.as_mut()) else { return Outcome::Ready(Ok(json!(false))) };
                let message: Value = serde_json::from_str(arg(1).as_str().unwrap_or("null")).unwrap_or(Value::Null);
                webview.send("message", json!({ "message": message, "transfer": [] }));
                return Outcome::Ready(Ok(json!(true)));
            }
            _ => {}
        }
        Outcome::done()
    }

    /// `$createWebviewPanel`: the panel opens pinned in the active group and, unless `preserveFocus`, takes the focus.
    // shortcut: one editor group, so every view column opens in it; ViewColumn.Beside needs the editor groups row.
    fn create_webview_panel(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) {
        let (extension, handle, view_type, init, show) = (&args[0], args[1].as_str().unwrap_or_default(), args[2].as_str().unwrap_or_default(), &args[3], &args[4]);
        let extension_id = extension["id"]["value"].as_str().or(extension["id"].as_str()).unwrap_or_default().to_owned();
        let path = PathBuf::from(format!("webview-panel/webview-undefined-{handle}"));
        let document = TextDocument::from_text(WEBVIEW_PANEL_SCHEME, &path, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
        self.webview_panels.panels.push(WebviewPanel {
            handle: handle.to_owned(),
            path,
            view_type: view_type.to_owned(),
            title: init["title"].as_str().unwrap_or_default().to_owned(),
            origin: webview_origin(view_type, &extension_id),
            extension_id,
            html: String::new(),
            content_options: init["webviewOptions"].clone(),
            retain_context_when_hidden: init["panelOptions"]["retainContextWhenHidden"] == true,
            state: Value::Null,
            icon: None,
            webview: None,
        });
        self.editors.open(document, false);
        self.editors_changed(cx);
        if show["preserveFocus"] != true {
            self.focus_webview_panel(handle, window, cx);
        }
    }

    /// The body of a webview editor: an empty box the native view is placed over once laid out.
    pub(super) fn webview_editor_body(&self, editor: &OpenEditor) -> Div {
        let view = self.webview_panel(&editor.document.path).and_then(|panel| panel.webview.as_ref()).map(Webview::view);
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
        let active = self.editors.active().map(|editor| editor.document.path.clone());
        // Native views draw over GPUI's layer: a quick input or menu on top of the webview would be hidden.
        // shortcut: the webview hides under these overlays, where Electron draws them over its iframe.
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        let gpui_focused = window.focused(cx).is_some();
        let mut created = Vec::new();
        for panel in &mut self.webview_panels.panels {
            let shown = active.as_deref() == Some(panel.path.as_path());
            if shown && panel.webview.is_none() {
                created.push(panel.handle.clone());
            }
            match (&panel.webview, shown) {
                (Some(webview), true) if covered => webview.hide(),
                (Some(webview), true) if gpui_focused => webview.release_keyboard(),
                (Some(webview), false) if panel.retain_context_when_hidden => webview.hide(),
                (Some(_), false) => panel.webview = None,
                _ => {}
            }
        }
        for handle in created {
            self.create_webview(&handle, window, cx);
        }
    }

    fn create_webview(&mut self, handle: &str, window: &mut Window, cx: &mut Context<Self>) {
        let events = self.webview_events(window, cx);
        let pre = self.app_root.join("src/vs/workbench/contrib/webview/browser/pre");
        let styles = self.webview_styles();
        let panel = self.webview_panel_mut(handle).expect("a shown panel");
        let mut webview = Webview::new(window, &panel.handle, &panel.origin, &panel.extension_id, pre, local_resource_roots(&panel.content_options), events);
        webview.send("styles", styles);
        panel.webview = Some(webview);
        panel.update_content();
    }

    /// The channel the webviews' host pages post on, read on the window's executor.
    fn webview_events(&mut self, window: &mut Window, cx: &mut Context<Self>) -> UnboundedSender<WebviewEvent> {
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
        let Some(panel) = self.webview_panel_mut(&event.id) else { return };
        let data = event.data;
        match event.channel.as_str() {
            "webview-ready" => {
                if let Some(webview) = &mut panel.webview {
                    webview.did_become_ready();
                }
            }
            "onmessage" => {
                // serializeWebviewMessage: the message as JSON text, ArrayBuffers by reference.
                let message = data["message"].to_string();
                self.host.send("ExtHostWebviews", "$onMessage", vec![json!(event.id), json!(message), json!([])]);
            }
            "do-update-state" => panel.state = data,
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
            "fatal-error" => eprintln!("[webview] {}: {}", event.id, data["message"].as_str().unwrap_or_default()),
            _ => {}
        }
    }

    /// mainThreadWebviews.ts `onDidClickLink`: `isSupportedLink`, then the opener service.
    // shortcut: `vscode:` links are dropped, the port has no URL handler service for them.
    fn open_webview_link(&mut self, link: &str, enable_command_uris: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (scheme, rest) = link.split_once(':').unwrap_or_default();
        match scheme {
            "http" | "https" | "mailto" => cx.open_url(link),
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
    fn focus_webview_panel(&mut self, handle: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_webview_panels(window, cx);
        window.blur(cx);
        if let Some(webview) = self.webview_panel_mut(handle).and_then(|panel| panel.webview.as_mut()) {
            webview.focus();
        }
    }

    /// Run from `editors_changed`: panels whose editor closed are disposed, tabs get titles, and the host hears
    /// which panel became active (`updateWebviewViewStates`).
    pub(super) fn sync_webview_panels(&mut self) {
        let editors = &self.editors;
        let (open, closed): (Vec<WebviewPanel>, Vec<WebviewPanel>) =
            std::mem::take(&mut self.webview_panels.panels).into_iter().partition(|panel| editors.editors.iter().any(|editor| editor.document.is(WEBVIEW_PANEL_SCHEME, &panel.path)));
        self.webview_panels.panels = open;
        for panel in closed {
            self.host.send("ExtHostWebviewPanels", "$onDidDisposeWebviewPanel", vec![json!(panel.handle)]);
        }
        self.host.webview_tabs = self
            .webview_panels
            .panels
            .iter()
            .map(|panel| (panel.path.clone(), WebviewTab { label: panel.title.clone(), view_type: format!("mainThreadWebview-{}", panel.view_type) }))
            .collect();
        let active_path = self.editors.active().map(|editor| editor.document.path.clone());
        let active = self.webview_panels.panels.iter().find(|panel| active_path.as_ref() == Some(&panel.path)).map(|panel| panel.handle.clone());
        if active == self.webview_panels.active {
            return;
        }
        self.webview_panels.active = active.clone();
        if self.webview_panels.panels.is_empty() {
            return;
        }
        let states: serde_json::Map<String, Value> = self
            .webview_panels
            .panels
            .iter()
            .map(|panel| {
                let shown = active.as_ref() == Some(&panel.handle);
                (panel.handle.clone(), json!({ "visible": shown, "active": shown, "position": 1 }))
            })
            .collect();
        self.host.send("ExtHostWebviewPanels", "$onDidChangeWebviewPanelViewStates", vec![Value::Object(states)]);
    }

    /// Sends the theme to every webview, as `WebviewThemeDataProvider.onThemeDataChanged` does.
    pub(super) fn restyle_webviews(&mut self) {
        let styles = self.webview_styles();
        for webview in self.webview_panels.panels.iter_mut().filter_map(|panel| panel.webview.as_mut()) {
            webview.send("styles", styles.clone());
        }
    }

    /// The title of a webview editor's tab, `WebviewInput.getName`.
    pub(super) fn webview_title(&self, path: &Path) -> Option<String> {
        self.webview_panel(path).map(|panel| panel.title.clone())
    }

    fn webview_styles(&self) -> Value {
        let label = self.color_themes.iter().find(|entry| entry.settings_id == self.theme.settings_id).map_or(self.theme.settings_id.clone(), |entry| entry.label.clone());
        webview::theme_data(&self.theme, &label, &self.setting("editor"), self.setting("accessibility.underlineLinks") == true)
    }

    /// webviewIconManager.ts: the dark icon for dark and high contrast themes, the light one otherwise.
    pub(super) fn webview_icon(&self, path: &Path) -> Option<&Path> {
        let (light, dark) = self.webview_panel(path)?.icon.as_ref()?;
        Some(match self.theme.kind {
            ThemeKind::Dark | ThemeKind::HighContrastDark => dark,
            ThemeKind::Light | ThemeKind::HighContrastLight => light,
        })
    }

    fn webview_panel(&self, path: &Path) -> Option<&WebviewPanel> {
        self.webview_panels.panels.iter().find(|panel| panel.path == path)
    }

    fn webview_panel_mut(&mut self, handle: &str) -> Option<&mut WebviewPanel> {
        self.webview_panels.panels.iter_mut().find(|panel| panel.handle == handle)
    }

    fn webview_editor_index(&self, handle: &str) -> Option<usize> {
        let panel = self.webview_panels.panels.iter().find(|panel| panel.handle == handle)?;
        self.editors.editors.iter().position(|editor| editor.document.is(WEBVIEW_PANEL_SCHEME, &panel.path))
    }
}

impl WebviewPanel {
    /// webviewElement.ts `doUpdateContent`.
    fn update_content(&mut self) {
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
fn local_resource_roots(options: &Value) -> Vec<PathBuf> {
    let roots = options["localResourceRoots"].as_array().into_iter().flatten();
    roots.filter(|root| root["scheme"] == "file").filter_map(|root| root["path"].as_str()).map(PathBuf::from).collect()
}

/// A UUID-shaped origin that stays the same for a view type of an extension, as the stored one upstream does.
fn webview_origin(view_type: &str, extension_id: &str) -> String {
    let digest = Sha256::digest(format!("{extension_id}/{view_type}").as_bytes());
    let hex: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn percent_decode(text: &str) -> String {
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
fn keystroke(data: &Value) -> Option<Keystroke> {
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
