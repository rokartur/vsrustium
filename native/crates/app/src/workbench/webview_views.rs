//! mainThreadWebviewViews.ts with contrib/webviewView: the webview views extensions contribute to the Explorer, each a
//! pane under the folder's tree whose webview lives in the webview panels' list (handled by the same
//! MainThreadWebviews calls), resolved once the pane first shows and its provider is registered.

use std::collections::HashMap;
use std::path::PathBuf;

use exthost::scan::ScannedExtension;
use gpui::{ClickEvent, Context, Div, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::search_view::SideBarView;
use super::webview_panels::WebviewPanel;
use crate::commands::Outcome;

/// The scheme of a webview view's entry in the webview panels' list; its path is the view id.
pub const WEBVIEW_VIEW_SCHEME: &str = "webview-view";
/// webviewViewPane.ts `storageKeys.webviewState`.
const WEBVIEW_STATE_KEY: &str = "webviewState";

#[derive(Default)]
pub struct WebviewViews {
    /// `$registerWebviewViewProvider`: the extension and the `retainContextWhenHidden` option, by view type.
    providers: HashMap<String, (Value, bool)>,
    /// Each contributed view's pane, by view id.
    panes: HashMap<String, Pane>,
    next_handle: u64,
}

#[derive(Default)]
struct Pane {
    /// The header's toggle; until clicked, the contribution's `visibility`.
    collapsed: Option<bool>,
    /// `$setWebviewViewTitle`, shown in place of the contributed name.
    title: Option<String>,
    description: Option<String>,
    /// Set once `$resolveWebviewView` is sent; the webview is the panel entry with this handle.
    handle: Option<String>,
    /// The body visibility the extension host last heard.
    visible: bool,
}

/// A `contributes.views.explorer` entry of `type: webview`.
struct ViewContribution {
    id: String,
    name: String,
    collapsed: bool,
}

impl Workbench {
    pub(crate) fn handle_webview_views_request(&mut self, method: &str, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let text = |value: Value| value.as_str().map(str::to_owned);
        match method {
            "$registerWebviewViewProvider" => {
                let view_type = text(arg(1)).unwrap_or_default();
                let retain = arg(2)["retainContextWhenHidden"] == true;
                self.webview_views.providers.insert(view_type, (arg(0), retain));
                cx.notify();
            }
            "$unregisterWebviewViewProvider" => {
                self.webview_views.providers.remove(arg(0).as_str().unwrap_or_default());
            }
            "$setWebviewViewTitle" | "$setWebviewViewDescription" => {
                let handle = arg(0);
                let Some(pane) = self.webview_views.panes.values_mut().find(|pane| pane.handle.as_deref() == handle.as_str()) else { return Outcome::done() };
                match method {
                    "$setWebviewViewTitle" => pane.title = text(arg(1)),
                    _ => pane.description = text(arg(1)),
                }
                cx.notify();
            }
            "$show" => self.show_webview_view(arg(0).as_str().unwrap_or_default(), arg(1) == true, window, cx),
            // shortcut: the badge belongs on the Explorer's activity bar item, which draws no badges yet (milestone 11).
            _ => {}
        }
        Outcome::done()
    }

    /// webviewViewPane.ts `show`: `viewsService.openView`, which opens the Explorer and expands the pane.
    fn show_webview_view(&mut self, handle: &str, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view_id) = self.webview_views.panes.iter().find(|(_, pane)| pane.handle.as_deref() == Some(handle)).map(|(id, _)| id.clone()) else { return };
        self.open_webview_view(&view_id, preserve_focus, window, cx);
    }

    /// `viewsService.openView`: a pane not resolved yet takes the focus once its webview exists.
    fn open_webview_view(&mut self, view_id: &str, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.webview_views.panes.entry(view_id.to_owned()).or_default();
        pane.collapsed = Some(false);
        let handle = pane.handle.clone();
        self.side_bar_view = SideBarView::Explorer;
        cx.notify();
        match handle.filter(|_| !preserve_focus) {
            Some(handle) => self.focus_webview_panel(&handle, window, cx),
            None => self.update_webview_panels(window, cx),
        }
    }

    /// views.ts registers `<view id>.focus` for every contributed view (`viewsService.openView(id, true)`).
    pub(crate) fn register_webview_view_commands(&mut self) {
        for contribution in webview_view_contributions(&self.extensions) {
            let id = contribution.id;
            self.commands.register(format!("{id}.focus"), move |workbench, _, window, cx| {
                workbench.open_webview_view(&id, false, window, cx);
                Outcome::done()
            });
        }
    }

    /// Runs before the webview panels update each render (webviewViewPane.ts `updateTreeVisibility`): a pane whose body
    /// shows activates `onView:<id>` and is resolved once its provider is there; the host hears visibility changes.
    pub(super) fn update_webview_views(&mut self) {
        let container_visible = self.side_bar_view == SideBarView::Explorer && self.side_bar_visible();
        for contribution in webview_view_contributions(&self.extensions) {
            let pane = self.webview_views.panes.entry(contribution.id.clone()).or_default();
            let visible = container_visible && !pane.collapsed.unwrap_or(contribution.collapsed);
            if let Some(handle) = &pane.handle
                && pane.visible != visible
            {
                self.host.send("ExtHostWebviewViews", "$onDidChangeWebviewViewVisibility", vec![json!(handle), json!(visible)]);
            }
            pane.visible = visible;
            if visible && pane.handle.is_none() {
                self.resolve_webview_view(&contribution);
            }
        }
    }

    /// webviewViewPane.ts `activate`, then mainThreadWebviewViews.ts `resolve`.
    fn resolve_webview_view(&mut self, contribution: &ViewContribution) {
        drop(self.host.activate_by_event(&format!("onView:{}", contribution.id)));
        let Some((extension, retain)) = self.webview_views.providers.get(&contribution.id).cloned() else { return };
        self.webview_views.next_handle += 1;
        let handle = format!("webview-view-{}", self.webview_views.next_handle);
        let state_key = view_state_key(&contribution.id);
        let memento = self.workspace_storage.lock().unwrap().get(&state_key).and_then(|text| serde_json::from_str::<Value>(text).ok());
        let state = memento.map_or(Value::Null, |memento| memento[WEBVIEW_STATE_KEY].clone());
        let pane = self.webview_views.panes.get_mut(&contribution.id).expect("the pane of a contributed view");
        pane.handle = Some(handle.clone());
        let mut panel = WebviewPanel::new(handle.clone(), WEBVIEW_VIEW_SCHEME.into(), PathBuf::from(&contribution.id), &contribution.id, contribution.name.clone(), &extension, json!({}), retain, None);
        panel.state = state.clone();
        self.webview_panels.panels.push(panel);
        let parsed = state.as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null);
        let title = pane.title.clone().map_or(Value::Null, Value::String);
        self.host.send("ExtHostWebviewViews", "$resolveWebviewView", vec![json!(handle), json!(contribution.id), title, parsed]);
    }

    /// The view ids whose pane body shows, as the paths of their webview panel entries.
    pub(super) fn visible_webview_views(&self) -> Vec<PathBuf> {
        self.webview_views.panes.iter().filter(|(_, pane)| pane.visible).map(|(id, _)| PathBuf::from(id)).collect()
    }

    /// webviewViewPane.ts `onDidUpdateState`: the webview's `vscode.setState` text, in the view's workspace memento.
    pub(super) fn save_webview_view_state(&mut self, view_id: &str, state: &Value) {
        let memento = json!({ WEBVIEW_STATE_KEY: state });
        self.workspace_storage.lock().unwrap().set(&view_state_key(view_id), memento.to_string());
    }

    /// The Explorer's contributed webview view panes, after its own sections.
    pub(super) fn webview_view_panes(&self, cx: &mut Context<Self>) -> Vec<Div> {
        let mut panes = Vec::new();
        for contribution in webview_view_contributions(&self.extensions) {
            let pane = self.webview_views.panes.get(&contribution.id);
            let expanded = !pane.and_then(|pane| pane.collapsed).unwrap_or(contribution.collapsed);
            let title = pane.and_then(|pane| pane.title.clone()).unwrap_or(contribution.name.clone());
            let description = pane.and_then(|pane| pane.description.clone());
            let id = contribution.id.clone();
            let header = self
                .section_header(&title, expanded, true)
                .when_some(description, |header, description| {
                    header.child(div().ml(px(4.)).text_size(px(11.)).text_color(self.color("descriptionForeground")).child(description))
                })
                .id(format!("webview-view-header-{id}"))
                .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                    let pane = workbench.webview_views.panes.entry(id.clone()).or_default();
                    pane.collapsed = Some(expanded);
                    cx.notify();
                }));
            let handle = pane.and_then(|pane| pane.handle.as_deref());
            let body = expanded.then(|| self.webview_body(|panel| Some(panel.handle.as_str()) == handle));
            panes.push(div().flex().flex_col().when(expanded, |pane| pane.flex_1().min_h_0()).child(header).children(body));
        }
        panes
    }
}

fn view_state_key(view_id: &str) -> String {
    format!("memento/webviewView.{view_id}")
}

/// viewsExtensionPoint.ts: the `views.explorer` entries of `type: webview`.
// shortcut: views in other containers wait for contributed view containers (milestone 11); `when` is not evaluated.
// Every Explorer view starts collapsed, whatever its `visibility` (viewsExtensionPoint.ts `showCollapsed`).
fn webview_view_contributions(extensions: &[ScannedExtension]) -> Vec<ViewContribution> {
    let mut contributions = Vec::new();
    for extension in extensions {
        for view in extension.description["contributes"]["views"]["explorer"].as_array().into_iter().flatten() {
            let (Some(id), Some(name)) = (view["id"].as_str(), view["name"].as_str()) else { continue };
            if view["type"] != "webview" {
                continue;
            }
            contributions.push(ViewContribution { id: id.to_owned(), name: name.to_owned(), collapsed: true });
        }
    }
    contributions
}
