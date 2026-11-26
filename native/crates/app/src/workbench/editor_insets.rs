//! mainThreadCodeInsets.ts (the proposed `editorInsets` API): a webview in a view zone below a line of a text
//! editor, `heightInLines` lines high.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Window, canvas, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::webview_panels::local_resource_roots;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position};
use crate::host_sync::snapshot_id;
use crate::webview::{self, Webview, WebviewEvent};

/// The prefix of an inset webview's id, which routes its host page's events here.
pub(super) const INSET_ID_PREFIX: &str = "editor-inset-";

pub(super) struct EditorInset {
    handle: i64,
    /// The text editor (`snapshot_id`) and the group that shows it.
    editor_id: String,
    group: u32,
    zone_id: u32,
    extension_id: String,
    html: String,
    content_options: Value,
    /// Created when the zone first shows whole; kept while hidden, as the zone's DOM node keeps its iframe.
    pub(super) webview: Option<Webview>,
    /// Set by `update_editor_insets` for this frame: the zone shows whole in its group's editor.
    shown: bool,
}

impl Workbench {
    pub(crate) fn handle_editor_insets_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let handle = args.first().and_then(Value::as_i64).unwrap_or_default();
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        match method {
            "$createEditorInset" => self.create_editor_inset(handle, args, cx),
            "$disposeEditorInset" => {
                let Some(index) = self.inset_index(handle) else { return Outcome::error("Unknown inset") };
                let inset = self.webview_panels.insets.remove(index);
                self.remove_inset_zone(&inset);
                cx.notify();
            }
            "$setHtml" => {
                let Some(inset) = self.inset_mut(handle) else { return Outcome::error("Unknown inset") };
                inset.html = arg(1).as_str().unwrap_or_default().to_owned();
                inset.update_content();
            }
            "$setOptions" => {
                let Some(inset) = self.inset_mut(handle) else { return Outcome::error("Unknown inset") };
                inset.content_options = arg(1);
                if let Some(webview) = &inset.webview {
                    webview.set_local_resource_roots(local_resource_roots(&inset.content_options));
                }
                inset.update_content();
            }
            "$postMessage" => {
                let Some(inset) = self.inset_mut(handle) else { return Outcome::error("Unknown inset") };
                if let Some(webview) = &mut inset.webview {
                    webview.send("message", json!({ "message": arg(1).to_string(), "buffers": [] }));
                }
                return Outcome::Ready(Ok(json!(true)));
            }
            _ => {}
        }
        Outcome::done()
    }

    /// `$createEditorInset`: the zone goes below `afterLineNumber` of the group's editor whose id and model match;
    /// without one the host hears the inset disposed.
    fn create_editor_inset(&mut self, handle: i64, args: &[Value], cx: &mut Context<Self>) {
        let (editor_id, uri, line, height, options, extension_id) = (args[1].as_str().unwrap_or_default(), &args[2], args[3].as_u64().unwrap_or_default() as usize, args[4].as_f64().unwrap_or_default(), args[5].clone(), &args[6]);
        let path = PathBuf::from(uri["path"].as_str().unwrap_or_default());
        let scheme = uri["scheme"].as_str().unwrap_or_default();
        let group = self.groups_in_grid_order().into_iter().find(|&group| self.editors.group_active(group).is_some_and(|(editor, _)| snapshot_id(editor) == editor_id && editor.document().is(scheme, &path)));
        let Some(group) = group else {
            self.host.send("ExtHostEditorInsets", "$onDidDispose", vec![json!(handle)]);
            return;
        };
        let zone_id = self.with_group(group, |workbench| {
            let line_height = workbench.editors.viewport.line_height;
            let anchor = line.checked_sub(1).map(|line| Position::new(line, 0));
            workbench.editors.active_mut().expect("the group shows the editor").add_view_zone(anchor, height as f32 * line_height)
        });
        let extension_id = extension_id["value"].as_str().or(extension_id.as_str()).unwrap_or_default().to_owned();
        self.webview_panels.insets.push(EditorInset { handle, editor_id: editor_id.to_owned(), group, zone_id, extension_id, html: String::new(), content_options: options, webview: None, shown: false });
        cx.notify();
    }

    /// Run from `editors_changed`: an inset whose editor no longer shows its model is disposed (`onDidChangeModel`,
    /// `onDidDispose` of the code editor).
    pub(super) fn sync_editor_insets(&mut self) {
        let insets = std::mem::take(&mut self.webview_panels.insets);
        for inset in insets {
            let alive = self.editors.group_active(inset.group).is_some_and(|(editor, _)| snapshot_id(editor) == inset.editor_id);
            match alive {
                true => self.webview_panels.insets.push(inset),
                false => {
                    self.remove_inset_zone(&inset);
                    self.host.send("ExtHostEditorInsets", "$onDidDispose", vec![json!(inset.handle)]);
                }
            }
        }
    }

    /// Runs each render: the insets whose zone shows whole get their webview, the others hide.
    // shortcut: a native view cannot be clipped by the editor as the zone's iframe is, so an inset scrolled partly
    // out of its editor hides until it shows whole again.
    pub(super) fn update_editor_insets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        let mut created = Vec::new();
        for index in 0..self.webview_panels.insets.len() {
            let inset = &self.webview_panels.insets[index];
            let shown = !covered
                && self.editors.group_active(inset.group).is_some_and(|(editor, viewport)| {
                    let top = editor.view_zone_top(inset.zone_id, viewport).map(|top| top - editor.scroll.top);
                    let height = editor.view_zone(inset.zone_id).map_or(0., |zone| zone.height);
                    top.is_some_and(|top| top >= 0. && top + height <= viewport.height)
                });
            let inset = &mut self.webview_panels.insets[index];
            inset.shown = shown;
            match (&inset.webview, shown) {
                (None, true) => created.push(index),
                (Some(webview), false) => webview.hide(),
                _ => {}
            }
        }
        // gpui tests run off the main thread, where AppKit views cannot be made.
        if created.is_empty() || cfg!(test) {
            return;
        }
        let events = self.webview_events(window, cx);
        let pre = self.app_root.join("src/vs/workbench/contrib/webview/browser/pre");
        let styles = self.webview_styles();
        for index in created {
            let inset = &mut self.webview_panels.insets[index];
            let id = format!("{INSET_ID_PREFIX}{}", inset.handle);
            let mut view = Webview::new(window, &id, &id, &inset.extension_id, pre.clone(), local_resource_roots(&inset.content_options), events.clone());
            view.send("styles", styles.clone());
            inset.webview = Some(view);
            inset.update_content();
        }
    }

    /// The boxes the insets of `editor` place their webviews over, at their zones.
    pub(super) fn editor_inset_elements(&self, editor: &OpenEditor, content_left: f32) -> Vec<AnyElement> {
        let viewport = self.editors.viewport;
        let mut elements = Vec::new();
        for inset in self.webview_panels.insets.iter().filter(|inset| inset.shown && inset.group == self.editors.id && inset.editor_id == snapshot_id(editor)) {
            let (Some(top), Some(zone), Some(view)) = (editor.view_zone_top(inset.zone_id, viewport), editor.view_zone(inset.zone_id), inset.webview.as_ref().map(Webview::view)) else { continue };
            let place = canvas(move |bounds, window, _| webview::place(&view, window, bounds), |_, _, _, _| {}).size_full();
            elements.push(div().absolute().left(px(content_left)).right(px(14.)).top(px(top - editor.scroll.top)).h(px(zone.height)).child(place).into_any_element());
        }
        elements
    }

    /// webviewElement.ts events of an inset's host page; its messages go to `ExtHostEditorInsets`.
    pub(super) fn handle_editor_inset_event(&mut self, event: WebviewEvent) {
        let Some(handle) = event.id.strip_prefix(INSET_ID_PREFIX).and_then(|handle| handle.parse::<i64>().ok()) else { return };
        let Some(inset) = self.inset_mut(handle) else { return };
        match event.channel.as_str() {
            "webview-ready" => {
                if let Some(webview) = &mut inset.webview {
                    webview.did_become_ready();
                }
            }
            "onmessage" => {
                let message: Value = event.data["message"].as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
                self.host.send("ExtHostEditorInsets", "$onDidReceiveMessage", vec![json!(handle), message]);
            }
            _ => {}
        }
    }

    fn remove_inset_zone(&mut self, inset: &EditorInset) {
        if !self.groups_in_grid_order().contains(&inset.group) {
            return;
        }
        let editor_id = inset.editor_id.clone();
        self.with_group(inset.group, |workbench| {
            if let Some(editor) = workbench.editors.active_mut().filter(|editor| snapshot_id(editor) == editor_id) {
                editor.remove_view_zone(inset.zone_id);
            }
        });
    }

    fn inset_index(&self, handle: i64) -> Option<usize> {
        self.webview_panels.insets.iter().position(|inset| inset.handle == handle)
    }

    fn inset_mut(&mut self, handle: i64) -> Option<&mut EditorInset> {
        self.webview_panels.insets.iter_mut().find(|inset| inset.handle == handle)
    }
}

impl EditorInset {
    /// webviewElement.ts `doUpdateContent`, with the inset's `enableFindWidget: false` element options.
    fn update_content(&mut self) {
        let Some(webview) = &mut self.webview else { return };
        let allow_scripts = self.content_options["enableScripts"] == true;
        let allow_forms = self.content_options["enableForms"].as_bool().unwrap_or(allow_scripts);
        webview.send(
            "content",
            json!({
                "contents": self.html,
                "title": Value::Null,
                "options": { "allowMultipleAPIAcquire": false, "allowScripts": allow_scripts, "allowForms": allow_forms },
                "state": Value::Null,
                "cspSource": "'self' https://*.vscode-cdn.net",
                "confirmBeforeClose": "keyboardOnly",
            }),
        );
    }
}
