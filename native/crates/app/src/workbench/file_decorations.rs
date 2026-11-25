//! The main-thread side of `vscode.window.registerFileDecorationProvider` (mainThreadDecorations.ts) and the
//! decorated resource labels (labels.ts `fileDecorations`): the label's color and the badge after it.

use std::path::Path;

use gpui::{Context, Div, ParentElement, Rgba, Styled, div, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::decorations::{Decoration, DecorationData, MARKERS_PROVIDER};
use crate::markers::{ERROR, ReadOptions, WARNING};

#[cfg(test)]
mod tests;

impl Workbench {
    pub(crate) fn handle_decorations_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let handle = args.first().and_then(Value::as_u64).unwrap_or_default();
        let mut decorations = self.decorations.borrow_mut();
        match method {
            "$registerDecorationProvider" => decorations.register(handle),
            "$unregisterDecorationProvider" => decorations.unregister(handle),
            "$onDidChange" => {
                let paths = args.get(1).and_then(Value::as_array).map(|uris| uris.iter().filter_map(|uri| uri["path"].as_str().map(str::to_owned)).collect());
                decorations.changed(handle, paths);
            }
            _ => {}
        }
        drop(decorations);
        self.send_decoration_requests(cx);
        cx.notify();
        Outcome::done()
    }

    /// `DecorationRequestsQueue`: the resources labels asked for since the last frame, one `$provideDecorations`
    /// per provider.
    pub(super) fn send_decoration_requests(&mut self, cx: &mut Context<Self>) {
        let requests = self.decorations.borrow_mut().take_requests();
        for (handle, requests) in requests {
            let dtos: Vec<Value> = requests.iter().map(|(id, path)| json!({ "id": id, "uri": { "$mid": 1, "scheme": "file", "path": path } })).collect();
            let reply = self.host.call_cancellable("ExtHostDecorations", "$provideDecorations", vec![json!(handle), Value::Array(dtos)]);
            cx.spawn(async move |workbench, cx| {
                let Ok(Ok(reply)) = reply.await else { return };
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.decorations.borrow_mut().answered(handle, &requests, &reply);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// markersFileDecorations.ts `_updateEnablement`: the Problems provider while `problems.visibility` and
    /// `problems.decorations.enabled` are on.
    pub(super) fn update_markers_file_decorations(&mut self) {
        let enabled = self.setting("problems.visibility") != Value::Bool(false) && self.setting("problems.decorations.enabled") != Value::Bool(false);
        let mut decorations = self.decorations.borrow_mut();
        if enabled == decorations.is_registered(MARKERS_PROVIDER) {
            return;
        }
        match enabled {
            true => decorations.register(MARKERS_PROVIDER),
            false => decorations.unregister(MARKERS_PROVIDER),
        }
        drop(decorations);
        if enabled {
            let resources: Vec<(String, std::path::PathBuf)> = self.diagnostics.uris.iter().filter_map(|(resource, uri)| match super::host_requests::uri_parts(uri) {
                Ok(("file", path)) => Some((resource.clone(), path)),
                _ => None,
            }).collect();
            for (resource, path) in resources {
                self.markers_file_decoration_changed(&resource, &path);
            }
        }
    }

    /// `MarkersDecorationsProvider.provideDecorations` after `onMarkerChanged`: the most severe error or warning
    /// colors the file, the count is its letter.
    pub(super) fn markers_file_decoration_changed(&self, resource: &str, path: &Path) {
        let mut decorations = self.decorations.borrow_mut();
        if !decorations.is_registered(MARKERS_PROVIDER) {
            return;
        }
        let markers = self.diagnostics.service.read(ReadOptions { resource: Some(resource), severities: Some(ERROR | WARNING), ..Default::default() });
        let data = markers.iter().map(|marker| marker.severity).max().map(|severity| DecorationData {
            weight: 100 * u32::from(severity),
            bubble: true,
            tooltip: Some(match markers.len() {
                1 => "1 problem in this file".to_owned(),
                count => format!("{count} problems in this file"),
            }),
            letter: Some(if markers.len() < 10 { markers.len().to_string() } else { "9+".to_owned() }),
            color: Some(if severity == ERROR { "list.errorForeground" } else { "list.warningForeground" }.to_owned()),
        });
        decorations.set(MARKERS_PROVIDER, &path.to_string_lossy(), data);
    }

    /// `getDecoration(uri, includeChildren)` for a file label; a folder includes its bubbling descendants.
    pub(super) fn file_decoration(&self, path: &Path, is_dir: bool) -> Option<Decoration> {
        self.decorations.borrow_mut().decoration(&path.to_string_lossy(), is_dir)
    }

    /// The label color of a decoration, when the label's `colors` option is on.
    pub(super) fn decoration_color(&self, decoration: Option<&Decoration>, colors: bool) -> Option<Rgba> {
        decoration.filter(|_| colors)?.color.as_deref().and_then(|color| self.theme.color(color))
    }

    /// iconlabel.css `.monaco-icon-label::after`: the letter at 90% size, weight 600 and 0.75 opacity, 5px after
    /// the label and 16px from the end, in the decoration's color; a folder with bubbling children shows the
    /// filled circle at 0.4 opacity instead (decorationsService.ts `_onlyChildren`).
    pub(super) fn decoration_badge(&self, decoration: Option<&Decoration>, foreground: Rgba) -> Option<Div> {
        let decoration = decoration?;
        let color = decoration.color.as_deref().and_then(|color| self.theme.color(color)).unwrap_or(foreground);
        if decoration.bubbled {
            return Some(self.icon("circle-filled", 14., 14., color).flex_none().ml(px(5.)).mr(px(14.)).opacity(0.4));
        }
        let letter = decoration.letter.clone()?;
        Some(div().flex_none().ml(px(5.)).mr(px(16.)).text_size(px(11.7)).font_weight(gpui::FontWeight::SEMIBOLD).opacity(0.75).text_color(color).child(letter))
    }
}
