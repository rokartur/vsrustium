//! The quick diff of the active editor (quickDiffService.ts, quickDiffModel.ts, quickDiffDecorator.ts): every quick
//! diff provider whose root holds the file names an original (the source controls' primary and secondary ones and
//! the extensions' `registerQuickDiffProvider` ones of MainThreadQuickDiff); the diffs against them draw in the
//! gutter, the overview ruler and the minimap.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui::{Bounds, Context, Div, Rgba, canvas, div, fill, point, prelude::*, px, size};
use serde_json::{Value, json};

use super::EditorMetrics;
use crate::commands::Outcome;
use crate::diff_editor::Side;
use crate::editor::OpenEditor;
use crate::workbench::Workbench;
use crate::workbench::host_requests::uri_parts;

/// quickDiffModel.ts `options.maxComputationTimeMs`.
const MAX_COMPUTATION_TIME_MS: u64 = 1000;
/// quickDiffService.ts `QuickDiffService.STORAGE_KEY` (profile scope).
const HIDDEN_PROVIDERS_KEY: &str = "workbench.scm.quickDiffProviders.hidden";

/// quickDiff.ts `ChangeType`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChangeKind {
    Modify,
    Add,
    Delete,
}

/// quickDiff.ts `QuickDiffProviderKind`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QuickDiffKind {
    Primary,
    Secondary,
    Contributed,
}

/// Where a provider's `getOriginalResource` goes.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// A source control's quick diff (`$provideOriginalResource`) or secondary one (`$provideSecondaryOriginalResource`).
    Scm(u64),
    /// `$registerQuickDiffProvider` of MainThreadQuickDiff, with its document selector.
    Contributed(u64, Value),
}

/// quickDiff.ts `QuickDiffProvider`.
#[derive(Clone, Debug, PartialEq)]
struct Provider {
    id: String,
    label: String,
    root: Option<PathBuf>,
    kind: QuickDiffKind,
    source: Source,
}

/// quickDiff.ts `QuickDiff`: a provider that named an original, its content loaded as a model.
#[derive(Clone, Debug, PartialEq)]
pub struct QuickDiff {
    pub id: String,
    pub label: String,
    pub kind: QuickDiffKind,
    pub original: Side,
}

/// A change as `toLineChanges` gives it (`IChange`): 1-based modified lines, `end` 0 for a deletion after
/// `start`; the original lines likewise, `original_end` 0 for an insertion after `original_start`. `provider`
/// indexes the entry's quick diffs; `modified` is `change2.modified`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuickDiffChange {
    pub kind: ChangeKind,
    pub start: usize,
    pub end: usize,
    pub original_start: usize,
    pub original_end: usize,
    pub provider: usize,
    pub modified: diff::LineRange,
}

/// One modified model's quick diff model.
#[derive(Default)]
pub(super) struct Entry {
    pub(super) quick_diffs: Vec<QuickDiff>,
    /// The quick diff generation the originals are for; a newer one asks again.
    pub(super) generation: Option<u64>,
    pub(super) pending: bool,
    /// The versions of the originals, then of the modified model, the changes are for.
    pub(super) versions: Vec<u32>,
    /// `changes`: the primary and contributed changes and the secondary ones that differ from the primary's, sorted.
    pub(super) changes: Vec<QuickDiffChange>,
    /// `allChanges`: with the secondary changes the primary ones repeat, as the API sees them.
    pub(super) all_changes: Vec<QuickDiffChange>,
    /// `changesVersionId`: the modified version the changes are for.
    pub(super) changes_version: u32,
}

#[derive(Default)]
pub struct QuickDiffs {
    pub(super) entries: HashMap<(String, PathBuf), Entry>,
    /// The quick diff peek of the active editor (`QuickDiffEditorController.widget`).
    pub(super) widget: Option<super::quick_diff_widget::PeekWidget>,
    /// Bumped on every source control or provider change, so the originals are asked for again.
    pub generation: u64,
    /// `$registerQuickDiffProvider` providers by handle.
    contributed: Vec<(u64, Provider)>,
    /// `$createSourceControlDiffInformation`: the model each handle follows and the last state it heard of.
    diff_information: HashMap<u64, ((String, PathBuf), Option<Value>)>,
    /// `hiddenQuickDiffProviders`, read from profile storage once.
    hidden: Option<HashSet<String>>,
}

/// `toLineChanges` of the diff's mappings, for the quick diff `provider`.
pub fn line_changes(diff: &diff::LinesDiff, provider: usize) -> Vec<QuickDiffChange> {
    let mut changes = Vec::new();
    for change in &diff.changes {
        let (original, modified) = (change.original, change.modified);
        let (original_start, original_end) = if original.is_empty() { (original.start - 1, 0) } else { (original.start, original.end - 1) };
        let (start, end) = if modified.is_empty() { (modified.start - 1, 0) } else { (modified.start, modified.end - 1) };
        let kind = match (original.is_empty(), modified.is_empty()) {
            (true, _) => ChangeKind::Add,
            (_, true) => ChangeKind::Delete,
            _ => ChangeKind::Modify,
        };
        changes.push(QuickDiffChange { kind, start, end, original_start, original_end, provider, modified });
    }
    changes
}

/// quickDiff.ts `compareChanges`.
fn compare_changes(a: &QuickDiffChange, b: &QuickDiffChange) -> std::cmp::Ordering {
    (a.start, a.end, a.original_start, a.original_end).cmp(&(b.start, b.end, b.original_start, b.original_end))
}

/// `LineRange.intersectsOrTouches`.
fn touches(a: diff::LineRange, b: diff::LineRange) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// quickDiffModel.ts `diff`: the changes of every quick diff against `modified`; a secondary change that repeats a
/// primary one (same modified lines, same original length and text) goes only into `all_changes`.
fn compute_changes(quick_diffs: &[(QuickDiffKind, &[String])], modified: &[String], options: diff::DiffOptions) -> (Vec<QuickDiffChange>, Vec<QuickDiffChange>) {
    let primary = quick_diffs.iter().position(|(kind, _)| *kind == QuickDiffKind::Primary);
    let mut diffs: Vec<QuickDiffChange> = Vec::new();
    let mut secondary_diffs = Vec::new();
    for (index, (kind, original)) in quick_diffs.iter().enumerate() {
        let computed = diff::compute_diff(original, modified, options);
        for (change, mapping) in line_changes(&computed, index).into_iter().zip(&computed.changes) {
            if let (Some(primary), QuickDiffKind::Secondary) = (primary, kind) {
                let repeated = diffs.iter().find(|other| other.provider == primary && other.modified == mapping.modified && original_len(other) == mapping.original.len());
                let same_text = repeated.is_some_and(|other| original_lines(quick_diffs[primary].1, other) == &original[mapping.original.start - 1..mapping.original.end - 1]);
                if same_text {
                    secondary_diffs.push(change);
                    continue;
                }
            }
            diffs.push(change);
        }
    }
    diffs.sort_by(compare_changes);
    let mut all: Vec<QuickDiffChange> = diffs.iter().copied().chain(secondary_diffs).collect();
    all.sort_by(compare_changes);
    (diffs, all)
}

fn original_len(change: &QuickDiffChange) -> usize {
    if change.original_end == 0 { 0 } else { change.original_end - change.original_start + 1 }
}

fn original_lines<'a>(lines: &'a [String], change: &QuickDiffChange) -> &'a [String] {
    if change.original_end == 0 { &[] } else { &lines[change.original_start - 1..change.original_end] }
}

/// quickDiffService.ts `createProviderComparer` with `providerComparer`: providers whose root holds the file first,
/// then primary, secondary, contributed.
fn sort_providers(providers: &mut [Provider]) {
    let rank = |provider: &Provider| {
        let kind = match provider.kind {
            QuickDiffKind::Primary => 0,
            QuickDiffKind::Secondary => 1,
            QuickDiffKind::Contributed => 2,
        };
        (provider.root.is_none(), kind)
    };
    providers.sort_by_key(rank);
}

impl Workbench {
    fn quick_diff_enabled(&self) -> bool {
        self.setting("scm.diffDecorations") != json!("none")
    }

    pub(in crate::workbench) fn register_quick_diff_visibility_commands(&mut self) {
        self.commands.register("_quickDiff.toggleProviderVisibility", |workbench, args, _, cx| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                workbench.toggle_quick_diff_provider_visibility(id);
            }
            cx.notify();
            Outcome::done()
        });
    }

    /// `MainThreadQuickDiff` requests.
    pub(crate) fn handle_quick_diff_request(&mut self, method: &str, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let handle = args.first().and_then(Value::as_u64).unwrap_or_default();
        match method {
            // `$registerQuickDiffProvider(handle, selector, id, label, rootUri)`.
            "$registerQuickDiffProvider" => {
                let text = |index: usize| args.get(index).and_then(Value::as_str).unwrap_or_default().to_string();
                let root = args.get(4).and_then(|uri| uri_parts(uri).ok()).map(|(_, path)| path);
                let selector = args.get(1).cloned().unwrap_or(Value::Null);
                let provider = Provider { id: text(2), label: text(3), root, kind: QuickDiffKind::Contributed, source: Source::Contributed(handle, selector) };
                self.quick_diffs.contributed.push((handle, provider));
            }
            "$unregisterQuickDiffProvider" => self.quick_diffs.contributed.retain(|(registered, _)| *registered != handle),
            "$createSourceControlDiffInformation" => {
                let Some((scheme, path)) = args.get(1).and_then(|uri| uri_parts(uri).ok()) else { return Outcome::done() };
                self.quick_diffs.diff_information.insert(handle, ((scheme.to_string(), path), None));
                self.send_diff_information();
                return Outcome::done();
            }
            "$disposeSourceControlDiffInformation" => {
                self.quick_diffs.diff_information.remove(&handle);
                return Outcome::done();
            }
            _ => return Outcome::done(),
        }
        self.quick_diffs.generation += 1;
        cx.notify();
        Outcome::done()
    }

    /// `quickDiffService.providers` for a file: the source controls' and the extensions' providers whose root holds
    /// it, in `createProviderComparer` order.
    fn quick_diff_providers(&self, scheme: &str, path: &std::path::Path) -> Vec<Provider> {
        let mut providers = Vec::new();
        for provider in &self.scm.model.providers {
            let root = provider.root_uri["path"].as_str().map(PathBuf::from);
            let features = [(provider.has_quick_diff_provider, QuickDiffKind::Primary, "quickDiffProvider", &provider.quick_diff_label), (provider.has_secondary_quick_diff_provider, QuickDiffKind::Secondary, "secondaryQuickDiffProvider", &provider.secondary_quick_diff_label)];
            for (enabled, kind, suffix, label) in features {
                if enabled {
                    let label = label.clone().unwrap_or_else(|| provider.label.clone());
                    providers.push(Provider { id: format!("{}.{suffix}", provider.id), label, root: root.clone(), kind, source: Source::Scm(provider.handle) });
                }
            }
        }
        providers.extend(self.quick_diffs.contributed.iter().map(|(_, provider)| provider.clone()));
        // `isEqualOrParent(uri, provider.rootUri)`; a source control root is a file URI.
        providers.retain(|provider| provider.root.as_ref().is_none_or(|root| scheme == "file" && path.starts_with(root)));
        sort_providers(&mut providers);
        providers
    }

    /// quickDiffModel.ts `triggerDiff` for the active editor: asks the providers for their originals once per
    /// generation, then diffs the models whenever a version moves.
    pub(in crate::workbench) fn update_quick_diff(&mut self, cx: &mut Context<Self>) {
        // The model each tab edits: a diff or merge editor's is not the tab entry's own.
        let open: Vec<(String, PathBuf)> = (0..self.editors.editors.len()).map(|index| self.editors.tab_model(index)).map(|model| (model.document.scheme.clone(), model.document.path.clone())).collect();
        let followed: Vec<(String, PathBuf)> = self.quick_diffs.diff_information.values().map(|(key, _)| key.clone()).collect();
        self.quick_diffs.entries.retain(|key, _| open.contains(key) || followed.contains(key));
        self.follow_quick_diff_widget(None, cx);
        if !self.quick_diff_enabled() || self.editors.active_diff().is_some() {
            return;
        }
        let mut keys: Vec<(String, PathBuf)> = self.editors.active().map(|editor| (editor.document.scheme.clone(), editor.document.path.clone())).into_iter().collect();
        for key in followed {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for key in keys {
            self.update_quick_diff_of(key, cx);
        }
    }

    fn update_quick_diff_of(&mut self, key: (String, PathBuf), cx: &mut Context<Self>) {
        let generation = self.quick_diffs.generation;
        let entry = self.quick_diffs.entries.entry(key.clone()).or_default();
        if entry.pending {
            return;
        }
        let entry = &self.quick_diffs.entries[&key];
        let originals_loaded = entry.quick_diffs.iter().all(|quick_diff| self.editors.model(&quick_diff.original.scheme, &quick_diff.original.path).is_some());
        if entry.generation != Some(generation) || !originals_loaded {
            self.quick_diffs.entries.get_mut(&key).expect("inserted above").pending = true;
            return self.request_original_resources(key, generation, cx);
        }
        let Some(modified) = self.editors.model(&key.0, &key.1) else { return };
        let mut originals = Vec::new();
        for quick_diff in &entry.quick_diffs {
            let original = self.editors.model(&quick_diff.original.scheme, &quick_diff.original.path).expect("checked above");
            originals.push((quick_diff.kind, original));
        }
        let mut versions: Vec<u32> = originals.iter().map(|(_, original)| original.version_id).collect();
        versions.push(modified.version_id);
        if versions == entry.versions {
            return;
        }
        let ignore_trim_whitespace = match self.setting("scm.diffDecorationsIgnoreTrimWhitespace").as_str() {
            Some("inherit") => self.setting("diffEditor.ignoreTrimWhitespace") != json!(false),
            Some("true") => true,
            _ => false,
        };
        let options = diff::DiffOptions { ignore_trim_whitespace, max_computation_time_ms: MAX_COMPUTATION_TIME_MS, compute_moves: false, extend_to_subwords: false };
        let inputs: Vec<(QuickDiffKind, &[String])> = originals.iter().map(|(kind, original)| (*kind, original.document.lines.as_slice())).collect();
        let (changes, all_changes) = compute_changes(&inputs, &modified.document.lines, options);
        let changes_version = modified.version_id;
        let entry = self.quick_diffs.entries.get_mut(&key).expect("found above");
        entry.versions = versions;
        entry.all_changes = all_changes;
        entry.changes_version = changes_version;
        let old_changes = std::mem::replace(&mut entry.changes, changes);
        self.send_diff_information();
        self.follow_quick_diff_widget(Some(old_changes), cx);
    }

    /// quickDiffService.ts `getQuickDiffs`: each provider in order, a contributed one only when its selector scores
    /// the file, answers with an original; each original's content loads as a model (textModelResolverService).
    fn request_original_resources(&mut self, key: (String, PathBuf), generation: u64, cx: &mut Context<Self>) {
        let uri = crate::editor::uri_value(&key.0, &key.1);
        let language = self.editors.model(&key.0, &key.1).map(|model| model.document.language_id.clone()).unwrap_or_default();
        let mut answers = Vec::new();
        for provider in self.quick_diff_providers(&key.0, &key.1) {
            let answer = match &provider.source {
                Source::Scm(handle) if provider.kind == QuickDiffKind::Secondary => self.host.call("ExtHostSCM", "$provideSecondaryOriginalResource", vec![json!(handle), uri.clone(), Value::Null]),
                Source::Scm(handle) => self.host.call("ExtHostSCM", "$provideOriginalResource", vec![json!(handle), uri.clone(), Value::Null]),
                Source::Contributed(handle, selector) => {
                    if crate::language_features::score(selector, &uri, &language, true) == 0 {
                        continue;
                    }
                    self.host.call("ExtHostQuickDiff", "$provideOriginalResource", vec![json!(handle), uri.clone(), Value::Null])
                }
            };
            answers.push((provider, answer));
        }
        cx.spawn(async move |workbench, cx| {
            let mut quick_diffs = Vec::new();
            for (provider, answer) in answers {
                // `Promise.allSettled`: a provider that fails or names nothing is left out.
                let Some(original) = answer.await.ok().and_then(Result::ok).filter(|uri| !uri.is_null()) else { continue };
                let Ok(content) = workbench.update(cx, |workbench, _| workbench.host.call("workbench", "resolveTextContent", vec![original.clone()])) else { return };
                let Some(content) = content.await.ok().and_then(Result::ok) else { continue };
                let Ok(Some(side)) = workbench.update(cx, |workbench, cx| {
                    let (scheme, path) = uri_parts(&original).ok()?;
                    let side = Side { scheme: scheme.to_string(), path };
                    let loaded = workbench.editors.model(&side.scheme, &side.path).is_some() || workbench.try_open_document(&original, &Value::Null, &content, false, cx).is_ok();
                    loaded.then_some(side)
                }) else {
                    continue;
                };
                quick_diffs.push(QuickDiff { id: provider.id, label: provider.label, kind: provider.kind, original: side });
            }
            let _ = workbench.update(cx, |workbench, cx| {
                if let Some(entry) = workbench.quick_diffs.entries.get_mut(&key) {
                    *entry = Entry { quick_diffs, generation: Some(generation), ..Entry::default() };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// mainThreadQuickDiff.ts `sendSourceControlDiffInformation`: the primary quick diff's changes as
    /// `[originalStart, originalEndExclusive, modifiedStart, modifiedEndExclusive]`, sent when they change.
    fn send_diff_information(&mut self) {
        let handles: Vec<u64> = self.quick_diffs.diff_information.keys().copied().collect();
        for handle in handles {
            let (key, sent) = &self.quick_diffs.diff_information[&handle];
            let entry = self.quick_diffs.entries.get(key);
            let primary = entry.and_then(|entry| entry.quick_diffs.iter().position(|quick_diff| quick_diff.kind == QuickDiffKind::Primary).map(|index| (entry, index)));
            let information = primary.map(|(entry, index)| {
                let changes: Vec<Value> = entry
                    .all_changes
                    .iter()
                    .filter(|change| change.provider == index)
                    .map(|change| {
                        let original_start = change.original_start + usize::from(change.original_end == 0);
                        json!([original_start, original_start + original_len(change), change.modified.start, change.modified.end])
                    })
                    .collect();
                let original = &entry.quick_diffs[index].original;
                json!({ "documentVersion": entry.changes_version, "original": crate::editor::uri_value(&original.scheme, &original.path), "modified": crate::editor::uri_value(&key.0, &key.1), "changes": changes })
            });
            if sent.as_ref() == Some(&information.clone().unwrap_or(Value::Null)) {
                continue;
            }
            self.host.send("ExtHostQuickDiff", "$acceptSourceControlDiffInformation", vec![json!(handle), information.clone().unwrap_or(Value::Null)]);
            self.quick_diffs.diff_information.get_mut(&handle).expect("listed above").1 = Some(information.unwrap_or(Value::Null));
        }
    }

    fn hidden_quick_diff_providers(&mut self) -> &mut HashSet<String> {
        let storage = self.profile_storage.clone();
        self.quick_diffs.hidden.get_or_insert_with(|| storage.lock().unwrap().get(HIDDEN_PROVIDERS_KEY).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default())
    }

    /// quickDiffService.ts `isQuickDiffProviderVisible`.
    pub(super) fn quick_diff_provider_visible(&self, id: &str) -> bool {
        match &self.quick_diffs.hidden {
            Some(hidden) => !hidden.contains(id),
            None => self.profile_storage.lock().unwrap().get(HIDDEN_PROVIDERS_KEY).and_then(|raw| serde_json::from_str::<HashSet<String>>(raw).ok()).is_none_or(|hidden| !hidden.contains(id)),
        }
    }

    /// quickDiffService.ts `toggleQuickDiffProviderVisibility` with `saveState`.
    fn toggle_quick_diff_provider_visibility(&mut self, id: &str) {
        let hidden = self.hidden_quick_diff_providers();
        if !hidden.remove(id) {
            hidden.insert(id.to_string());
        }
        let raw = (!hidden.is_empty()).then(|| serde_json::to_string(&hidden.iter().collect::<Vec<_>>()).expect("strings serialize"));
        let mut storage = self.profile_storage.lock().unwrap();
        match raw {
            Some(raw) => storage.set(HIDDEN_PROVIDERS_KEY, raw),
            None => storage.delete(HIDDEN_PROVIDERS_KEY),
        }
    }

    /// The quick diff model of an editor's file.
    pub(super) fn quick_diff_entry(&self, editor: &OpenEditor) -> Option<&Entry> {
        self.quick_diffs.entries.get(&(editor.document.scheme.clone(), editor.document.path.clone()))
    }

    /// The model's `changes`, empty without a quick diff.
    pub(super) fn quick_diff_changes(&self, editor: &OpenEditor) -> &[QuickDiffChange] {
        self.quick_diff_entry(editor).map_or(&[], |entry| entry.changes.as_slice())
    }

    /// quickDiffDecorator.ts `onDidChange`: the changes of visible quick diffs, without the non-primary ones touching
    /// a primary change, each with whether it is drawn as primary (`primary` or `contributed`).
    fn quick_diff_decorations(&self, editor: &OpenEditor) -> Vec<(QuickDiffChange, bool)> {
        let Some(entry) = self.quick_diff_entry(editor) else { return Vec::new() };
        let primary = entry.quick_diffs.iter().position(|quick_diff| quick_diff.kind == QuickDiffKind::Primary);
        let mut decorations = Vec::new();
        for change in &entry.changes {
            let quick_diff = &entry.quick_diffs[change.provider];
            if !self.quick_diff_provider_visible(&quick_diff.id) {
                continue;
            }
            let overlaps_primary = entry.changes.iter().any(|other| Some(other.provider) == primary && touches(other.modified, change.modified));
            if quick_diff.kind != QuickDiffKind::Primary && overlaps_primary {
                continue;
            }
            decorations.push((*change, quick_diff.kind != QuickDiffKind::Secondary));
        }
        decorations
    }

    /// dirtydiffDecorator.css: the `dirty-diff-glyph`s of line `index`, 5px into the line decorations lane: a bar
    /// `scm.diffDecorationsGutterWidth` wide (striped per `scm.diffDecorationsGutterPattern`) for added and modified
    /// lines, a 4px triangle on the line's bottom edge for deleted ones, in the secondary colors for a secondary quick
    /// diff. With `scm.diffDecorationsGutterAction` `diff` a glyph is a pointer that opens the change's peek, and on
    /// hover its `:before` grows 6px to the left.
    pub(super) fn quick_diff_glyph(&self, editor: &OpenEditor, index: usize, metrics: EditorMetrics, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let setting = self.setting("scm.diffDecorations");
        if !matches!(setting.as_str(), Some("all" | "gutter") | None) {
            return None;
        }
        let line = index + 1;
        let decorations: Vec<(QuickDiffChange, bool)> = self
            .quick_diff_decorations(editor)
            .into_iter()
            .filter(|(change, _)| match change.kind {
                ChangeKind::Delete => change.start.max(1) == line,
                _ => (change.start..=change.end).contains(&line),
            })
            .collect();
        if decorations.is_empty() {
            return None;
        }
        let left = metrics.glyph_margin_width() + metrics.line_numbers_width + 5.;
        let opacity = if self.setting("scm.diffDecorationsGutterVisibility") == json!("hover") { 0. } else { 1. };
        let width = self.setting("scm.diffDecorationsGutterWidth").as_f64().filter(|width| *width > 0. && *width <= 5.).unwrap_or(3.) as f32;
        let pattern = self.setting("scm.diffDecorationsGutterPattern");
        let gutter_background = self.color("editorGutter.background");
        let mut lane = div().absolute().top_0().left(px(left)).h(px(metrics.line_height)).w(px(width)).opacity(opacity);
        let mut tooltip = "";
        for (change, primary) in decorations {
            let glyph = match change.kind {
                ChangeKind::Delete => {
                    tooltip = "Removed lines";
                    let color = self.color(if primary { "editorGutter.deletedBackground" } else { "editorGutter.deletedSecondaryBackground" });
                    div().absolute().left_0().top(px(metrics.line_height - 4.)).child(deleted_triangle(color))
                }
                kind => {
                    let added = kind == ChangeKind::Add;
                    tooltip = if added { "Added lines" } else { "Changed lines" };
                    let key = match (added, primary) {
                        (true, true) => "editorGutter.addedBackground",
                        (true, false) => "editorGutter.addedSecondaryBackground",
                        (false, true) => "editorGutter.modifiedBackground",
                        (false, false) => "editorGutter.modifiedSecondaryBackground",
                    };
                    let color = self.color(key);
                    let striped = pattern[if added { "added" } else { "modified" }] == true;
                    let bar = |bar: Div| match striped {
                        true => bar.child(stripes(color, gutter_background, width, primary)),
                        false => bar.bg(color),
                    };
                    // `.pattern:before` sits 3px right of where a solid glyph's does.
                    let hover_left = if striped { -3. } else { -6. };
                    let hover_bar = bar(div().absolute().top_0().h_full().left(px(hover_left)).w(px(6.)).overflow_hidden()).opacity(0.).group_hover("quick-diff-glyph", |bar| bar.opacity(1.));
                    bar(div().absolute().inset_0().overflow_hidden()).child(hover_bar)
                }
            };
            lane = lane.child(glyph);
        }
        let lane = lane.id(("quick-diff-glyph", index)).child(self.hover_target(gpui::SharedString::from(format!("quick-diff-glyph-hover-{index}")), crate::workbench::hover::HoverContent::Text(tooltip.into()), crate::workbench::hover::HoverOptions::mouse()));
        if self.setting("scm.diffDecorationsGutterAction") == json!("none") {
            return Some(lane);
        }
        Some(lane.group("quick-diff-glyph").cursor_pointer().on_click(cx.listener(move |workbench, _: &gpui::ClickEvent, window, cx| {
            cx.stop_propagation();
            workbench.quick_diff_glyph_clicked(line, window, cx);
        })))
    }

    /// quickDiffDecorator.ts `overviewRuler` lanes (`OverviewRulerLane.Left`): 0-based inclusive line spans and colors.
    pub(super) fn quick_diff_ruler_lines(&self, editor: &OpenEditor) -> Vec<(usize, usize, Option<Rgba>)> {
        if !matches!(self.setting("scm.diffDecorations").as_str(), Some("all" | "overview") | None) {
            return Vec::new();
        }
        let color = |key: &str| self.theme.color(key);
        self.quick_diff_decorations(editor)
            .into_iter()
            .map(|(change, _)| match change.kind {
                ChangeKind::Delete => (change.start.max(1) - 1, change.start.max(1) - 1, color("editorOverviewRuler.deletedForeground")),
                ChangeKind::Add => (change.start - 1, change.end - 1, color("editorOverviewRuler.addedForeground")),
                ChangeKind::Modify => (change.start - 1, change.end - 1, color("editorOverviewRuler.modifiedForeground")),
            })
            .collect()
    }

    /// quickDiffDecorator.ts `minimap` decorations (`MinimapPosition.Gutter`): 0-based inclusive line spans and colors.
    pub(in crate::workbench) fn quick_diff_minimap_lines(&self, editor: &OpenEditor) -> Vec<(usize, usize, Rgba)> {
        if !matches!(self.setting("scm.diffDecorations").as_str(), Some("all" | "minimap") | None) {
            return Vec::new();
        }
        let mut lines = Vec::new();
        for (change, _) in self.quick_diff_decorations(editor) {
            let (start, end, key) = match change.kind {
                ChangeKind::Delete => (change.start.max(1), change.start.max(1), "minimapGutter.deletedBackground"),
                ChangeKind::Add => (change.start, change.end, "minimapGutter.addedBackground"),
                ChangeKind::Modify => (change.start, change.end, "minimapGutter.modifiedBackground"),
            };
            if let Some(color) = self.theme.color(key) {
                lines.push((start - 1, end - 1, color));
            }
        }
        lines
    }
}

/// `.dirty-diff-deleted:after`: 4px transparent top and bottom borders around a 4px left border, drawn as the
/// right-pointing triangle they make, one 1px column per step.
fn deleted_triangle(color: Rgba) -> Div {
    div().flex().items_center().h(px(8.)).children((0..4).map(move |column| div().w(px(1.)).h(px(8. - 2. * column as f32)).bg(color)))
}

/// `.dirty-diff-*.pattern`: a `width` square tile of a 45 degree `linear-gradient` (-45deg for a primary quick diff),
/// the color on its first and third quarters and the gutter background between, repeated down the line.
fn stripes(color: Rgba, background: Rgba, width: f32, primary: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let rows = (f32::from(bounds.size.height)).ceil() as usize;
            for y in 0..rows {
                for x in 0..width.ceil() as usize {
                    let (tile_x, tile_y) = (x as f32 + 0.5, (y as f32 + 0.5) % width);
                    let position = if primary { 1. - (tile_x + tile_y) / (2. * width) } else { 0.5 + (tile_x - tile_y) / (2. * width) };
                    let quarter = (position.clamp(0., 0.999) * 4.) as usize;
                    let color_at = if quarter % 2 == 0 { color } else { background };
                    let origin = bounds.origin + point(px(x as f32), px(y as f32));
                    window.paint_quad(fill(Bounds::new(origin, size(px(1.), px(1.))), color_at));
                }
            }
        },
    )
    .absolute()
    .inset_0()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    const OPTIONS: diff::DiffOptions = diff::DiffOptions { ignore_trim_whitespace: false, max_computation_time_ms: 0, compute_moves: false, extend_to_subwords: false };

    fn change(kind: ChangeKind, (start, end): (usize, usize), (original_start, original_end): (usize, usize), provider: usize, modified: (usize, usize)) -> QuickDiffChange {
        QuickDiffChange { kind, start, end, original_start, original_end, provider, modified: diff::LineRange { start: modified.0, end: modified.1 } }
    }

    #[test]
    fn line_changes_follow_to_line_changes() {
        let changes = line_changes(&diff::compute_diff(&lines("a\nb\nc\nd\ne"), &lines("a\nB\nc\nnew\nd"), OPTIONS), 0);
        assert_eq!(
            changes,
            vec![
                change(ChangeKind::Modify, (2, 2), (2, 2), 0, (2, 3)),
                change(ChangeKind::Add, (4, 4), (3, 0), 0, (4, 5)),
                change(ChangeKind::Delete, (5, 0), (5, 5), 0, (6, 6)),
            ]
        );
    }

    #[test]
    fn quick_diff_secondary_changes_that_repeat_the_primary_stay_out_of_changes() {
        // HEAD has `b`, the index stages `B` on line 2; the working tree also edits line 4.
        let (head, index, modified) = (lines("a\nb\nc\nd"), lines("a\nB\nc\nd"), lines("a\nB\nc\nD"));
        let (changes, all) = compute_changes(&[(QuickDiffKind::Primary, &index), (QuickDiffKind::Secondary, &head)], &modified, OPTIONS);
        // The index diff has line 4; the HEAD diff has lines 2 and 4, and its line 4 repeats the primary's.
        assert_eq!(changes.iter().map(|change| (change.start, change.provider)).collect::<Vec<_>>(), [(2, 1), (4, 0)]);
        assert_eq!(all.iter().map(|change| (change.start, change.provider)).collect::<Vec<_>>(), [(2, 1), (4, 0), (4, 1)]);
    }

    #[gpui::test]
    fn quick_diff_contributed_providers_and_diff_information_follow_main_thread_quick_diff(cx: &mut gpui::TestAppContext) {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-quick-diff-api-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.canonicalize().unwrap().join("a.md");
        std::fs::write(&file, "one\nTWO").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, crate::host_sync::HostSync::new(sender), cx));
        let uri = json!({ "scheme": "file", "path": file.to_string_lossy() });
        let original = json!({ "scheme": "ext", "path": file.to_string_lossy() });
        workbench.update_in(cx, |workbench, window, cx| {
            workbench.open_file(file.clone(), true, window, cx);
            drop(workbench.handle_quick_diff_request("$registerQuickDiffProvider", &[json!(7), json!([{ "pattern": "**/*.md" }]), json!("ext.diff"), json!("Ext"), Value::Null], cx));
            drop(workbench.handle_quick_diff_request("$registerQuickDiffProvider", &[json!(8), json!([{ "pattern": "**/*.ts" }]), json!("ext.ts"), json!("TS"), Value::Null], cx));
            drop(workbench.handle_quick_diff_request("$createSourceControlDiffInformation", &[json!(3), uri.clone()], cx));
            workbench.update_quick_diff(cx);
        });
        // Only the provider whose selector scores the file is asked.
        let mut sent: Vec<Value> = Vec::new();
        let mut asked: Vec<HostMessageArgs> = Vec::new();
        for message in receiver.try_iter() {
            match message.method {
                "$provideOriginalResource" => asked.push((message.proxy, message.args, message.reply)),
                "$acceptSourceControlDiffInformation" => sent.push(json!(message.args)),
                _ => {}
            }
        }
        assert_eq!(asked.iter().map(|(proxy, args, _)| (*proxy, args[0].clone())).collect::<Vec<_>>(), [("ExtHostQuickDiff", json!(7))]);
        for (_, _, reply) in asked {
            reply.unwrap().send(Ok(original.clone())).unwrap();
        }
        cx.run_until_parked();
        let resolve: Vec<_> = receiver.try_iter().filter(|message| message.method == "resolveTextContent").collect();
        for message in resolve {
            message.reply.unwrap().send(Ok(json!("one\ntwo"))).unwrap();
        }
        cx.run_until_parked();
        workbench.update_in(cx, |workbench, _, cx| workbench.update_quick_diff(cx));
        sent.extend(receiver.try_iter().filter(|message| message.method == "$acceptSourceControlDiffInformation").map(|message| json!(message.args)));
        // A contributed quick diff is not the primary one, so the information is undefined.
        assert_eq!(sent, [json!([3, null])]);
        let changes = workbench.read_with(cx, |workbench, _| workbench.quick_diff_changes(workbench.editors.active().unwrap()).iter().map(|change| (change.start, change.provider)).collect::<Vec<_>>());
        assert_eq!(changes, [(2, 0)]);
    }

    type HostMessageArgs = (&'static str, Vec<Value>, Option<futures::channel::oneshot::Sender<Result<Value, Value>>>);

    #[test]
    fn quick_diff_providers_sort_rooted_first_then_by_kind() {
        let provider = |id: &str, root: Option<&str>, kind| Provider { id: id.into(), label: id.into(), root: root.map(PathBuf::from), kind, source: Source::Scm(0) };
        let mut providers = vec![provider("c", None, QuickDiffKind::Contributed), provider("s", Some("/r"), QuickDiffKind::Secondary), provider("p", Some("/r"), QuickDiffKind::Primary)];
        sort_providers(&mut providers);
        assert_eq!(providers.iter().map(|provider| provider.id.as_str()).collect::<Vec<_>>(), ["p", "s", "c"]);
    }
}
