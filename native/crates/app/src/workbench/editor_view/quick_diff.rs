//! The quick diff of the active editor (quickDiffModel.ts, quickDiffDecorator.ts): the source control's
//! `provideOriginalResource` names the original, the diff against it draws in the gutter and the overview ruler.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{Context, Div, Rgba, div, prelude::*, px};
use serde_json::{Value, json};

use super::EditorMetrics;
use crate::workbench::Workbench;
use crate::diff_editor::Side;
use crate::editor::OpenEditor;
use crate::workbench::host_requests::uri_parts;

/// quickDiffModel.ts `options.maxComputationTimeMs`.
const MAX_COMPUTATION_TIME_MS: u64 = 1000;

/// quickDiff.ts `ChangeType`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChangeKind {
    Modify,
    Add,
    Delete,
}

/// A change as `toLineChanges` gives it: 1-based modified lines, `end` 0 for a deletion after `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuickDiffChange {
    pub kind: ChangeKind,
    pub start: usize,
    pub end: usize,
}

/// One modified model's quick diff.
#[derive(Default)]
struct Entry {
    /// The provider's answer: `None` when it names no original.
    original: Option<Side>,
    /// The SCM generation the answer is for; a newer one asks again.
    generation: Option<u64>,
    pending: bool,
    versions: (u32, u32),
    changes: Vec<QuickDiffChange>,
}

#[derive(Default)]
pub struct QuickDiffs {
    entries: HashMap<(String, PathBuf), Entry>,
    /// Bumped on every source control change, so the originals are asked for again.
    pub generation: u64,
}

/// `toLineChanges` of the diff's mappings.
pub fn line_changes(diff: &diff::LinesDiff) -> Vec<QuickDiffChange> {
    diff.changes
        .iter()
        .map(|change| {
            let (original, modified) = (change.original, change.modified);
            if original.is_empty() {
                QuickDiffChange { kind: ChangeKind::Add, start: modified.start, end: modified.end - 1 }
            } else if modified.is_empty() {
                QuickDiffChange { kind: ChangeKind::Delete, start: modified.start - 1, end: 0 }
            } else {
                QuickDiffChange { kind: ChangeKind::Modify, start: modified.start, end: modified.end - 1 }
            }
        })
        .collect()
}

impl Workbench {
    fn quick_diff_enabled(&self) -> bool {
        self.setting("scm.diffDecorations") != json!("none")
    }

    /// quickDiffModel.ts `triggerDiff` for the active editor: asks its source control for the original once per
    /// SCM change, then diffs the two models whenever either version moves.
    pub(in crate::workbench) fn update_quick_diff(&mut self, cx: &mut Context<Self>) {
        let open: Vec<(String, PathBuf)> = self.editors.editors.iter().map(|editor| (editor.document().scheme.clone(), editor.document().path.clone())).collect();
        self.quick_diffs.entries.retain(|key, _| open.contains(key));
        if !self.quick_diff_enabled() || self.editors.active_diff().is_some() {
            return;
        }
        let Some(editor) = self.editors.active() else { return };
        let key = (editor.document().scheme.clone(), editor.document().path.clone());
        let generation = self.quick_diffs.generation;
        let original_loaded = |workbench: &Self, entry: &Entry| entry.original.as_ref().is_none_or(|side| workbench.editors.model(&side.scheme, &side.path).is_some());
        let entry = self.quick_diffs.entries.entry(key.clone()).or_default();
        if entry.pending {
            return;
        }
        let entry = &self.quick_diffs.entries[&key];
        if entry.generation != Some(generation) || !original_loaded(self, entry) {
            self.quick_diffs.entries.get_mut(&key).expect("inserted above").pending = true;
            return self.request_original_resource(key, generation, cx);
        }
        let Some(original) = &entry.original else { return };
        let (Some(original), Some(modified)) = (self.editors.model(&original.scheme, &original.path), self.editors.model(&key.0, &key.1)) else { return };
        let versions = (original.text_model().version_id, modified.text_model().version_id);
        if versions == entry.versions {
            return;
        }
        let ignore_trim_whitespace = match self.setting("scm.diffDecorationsIgnoreTrimWhitespace").as_str() {
            Some("inherit") => self.setting("diffEditor.ignoreTrimWhitespace") != json!(false),
            Some("true") => true,
            _ => false,
        };
        let options = diff::DiffOptions { ignore_trim_whitespace, max_computation_time_ms: MAX_COMPUTATION_TIME_MS, compute_moves: false, extend_to_subwords: false };
        let changes = line_changes(&diff::compute_diff(&original.document().lines, &modified.document().lines, options));
        let entry = self.quick_diffs.entries.get_mut(&key).expect("found above");
        entry.versions = versions;
        entry.changes = changes;
    }

    /// mainThreadSCM.ts `getOriginalResource`: the first source control whose root holds the file answers
    /// `$provideOriginalResource`; its content loads as a model (textModelResolverService).
    fn request_original_resource(&mut self, key: (String, PathBuf), generation: u64, cx: &mut Context<Self>) {
        let uri = crate::editor::uri_value(&key.0, &key.1);
        let provider = self.scm.model.providers.iter().find(|provider| {
            let root = provider.root_uri["path"].as_str().unwrap_or_default();
            provider.has_quick_diff_provider && provider.root_uri["scheme"] == uri["scheme"] && key.1.starts_with(root)
        });
        let Some(handle) = provider.map(|provider| provider.handle) else {
            let entry = self.quick_diffs.entries.get_mut(&key).expect("inserted by the caller");
            *entry = Entry { generation: Some(generation), ..Entry::default() };
            return;
        };
        let answer = self.host.call("ExtHostSCM", "$provideOriginalResource", vec![json!(handle), uri, Value::Null]);
        cx.spawn(async move |workbench, cx| {
            let original = answer.await.ok().and_then(Result::ok).filter(|uri| !uri.is_null());
            let content = match &original {
                Some(uri) => {
                    let Ok(content) = workbench.update(cx, |workbench, _| workbench.host.call("workbench", "resolveTextContent", vec![uri.clone()])) else { return };
                    content.await.ok().and_then(Result::ok)
                }
                None => None,
            };
            let _ = workbench.update(cx, |workbench, cx| {
                let side = match (&original, content) {
                    (Some(uri), Some(content)) => uri_parts(uri).ok().and_then(|(scheme, path)| {
                        let side = Side { scheme: scheme.to_string(), path };
                        let loaded = workbench.editors.model(&side.scheme, &side.path).is_some() || workbench.try_open_document(uri, &Value::Null, &content, false, cx).is_ok();
                        loaded.then_some(side)
                    }),
                    _ => None,
                };
                if let Some(entry) = workbench.quick_diffs.entries.get_mut(&key) {
                    *entry = Entry { original: side, generation: Some(generation), ..Entry::default() };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The model's quick diff changes, `None` when its source control names no original (`getOriginalResource`).
    pub(in crate::workbench) fn quick_diff_line_changes(&self, editor: &OpenEditor) -> Option<&[QuickDiffChange]> {
        let key = (editor.document().scheme.clone(), editor.document().path.clone());
        self.quick_diffs.entries.get(&key).filter(|entry| entry.original.is_some()).map(|entry| entry.changes.as_slice())
    }

    /// The active editor's quick diff changes, empty when it has none.
    fn quick_diff_changes(&self, editor: &OpenEditor) -> &[QuickDiffChange] {
        let key = (editor.document().scheme.clone(), editor.document().path.clone());
        self.quick_diffs.entries.get(&key).map_or(&[], |entry| entry.changes.as_slice())
    }

    /// dirtydiffDecorator.css: the `dirty-diff-glyph` of line `index`, 5px into the line decorations lane: a bar
    /// `scm.diffDecorationsGutterWidth` wide for added and modified lines, a 4px triangle on the line's bottom
    /// edge for deleted ones.
    pub(super) fn quick_diff_glyph(&self, editor: &OpenEditor, index: usize, metrics: EditorMetrics) -> Option<Div> {
        let setting = self.setting("scm.diffDecorations");
        if !matches!(setting.as_str(), Some("all" | "gutter") | None) {
            return None;
        }
        let line = index + 1;
        let change = self.quick_diff_changes(editor).iter().find(|change| match change.kind {
            ChangeKind::Delete => change.start.max(1) == line,
            _ => (change.start..=change.end).contains(&line),
        })?;
        let left = metrics.glyph_margin_width() + metrics.line_numbers_width + 5.;
        let opacity = if self.setting("scm.diffDecorationsGutterVisibility") == json!("hover") { 0. } else { 1. };
        let glyph = div().absolute().left(px(left)).opacity(opacity);
        Some(match change.kind {
            ChangeKind::Delete => {
                let color = self.color("editorGutter.deletedBackground");
                glyph.top(px(metrics.line_height - 4.)).child(deleted_triangle(color))
            }
            kind => {
                let key = if kind == ChangeKind::Add { "editorGutter.addedBackground" } else { "editorGutter.modifiedBackground" };
                let width = self.setting("scm.diffDecorationsGutterWidth").as_f64().filter(|width| *width > 0. && *width <= 5.).unwrap_or(3.) as f32;
                glyph.top_0().h(px(metrics.line_height)).w(px(width)).bg(self.color(key))
            }
        })
    }

    /// quickDiffDecorator.ts `overviewRuler` lanes (`OverviewRulerLane.Left`): 0-based inclusive line spans and colors.
    pub(super) fn quick_diff_ruler_lines(&self, editor: &OpenEditor) -> Vec<(usize, usize, Option<Rgba>)> {
        if !matches!(self.setting("scm.diffDecorations").as_str(), Some("all" | "overview") | None) {
            return Vec::new();
        }
        let color = |key: &str| self.theme.color(key);
        self.quick_diff_changes(editor)
            .iter()
            .map(|change| match change.kind {
                ChangeKind::Delete => (change.start.max(1) - 1, change.start.max(1) - 1, color("editorOverviewRuler.deletedForeground")),
                ChangeKind::Add => (change.start - 1, change.end - 1, color("editorOverviewRuler.addedForeground")),
                ChangeKind::Modify => (change.start - 1, change.end - 1, color("editorOverviewRuler.modifiedForeground")),
            })
            .collect()
    }
}

/// `.dirty-diff-deleted:after`: 4px transparent top and bottom borders around a 4px left border, drawn as the
/// right-pointing triangle they make, one 1px column per step.
fn deleted_triangle(color: Rgba) -> Div {
    div().flex().items_center().h(px(8.)).children((0..4).map(move |column| div().w(px(1.)).h(px(8. - 2. * column as f32)).bg(color)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn line_changes_follow_to_line_changes() {
        let options = diff::DiffOptions { ignore_trim_whitespace: false, max_computation_time_ms: 0, compute_moves: false, extend_to_subwords: false };
        let original = lines("a\nb\nc\nd\ne");
        let modified = lines("a\nB\nc\nnew\nd");
        let changes = line_changes(&diff::compute_diff(&original, &modified, options));
        assert_eq!(
            changes,
            vec![
                QuickDiffChange { kind: ChangeKind::Modify, start: 2, end: 2 },
                QuickDiffChange { kind: ChangeKind::Add, start: 4, end: 4 },
                QuickDiffChange { kind: ChangeKind::Delete, start: 5, end: 0 },
            ]
        );
    }
}
