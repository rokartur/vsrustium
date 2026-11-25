//! workbench/contrib/folding with the `SyntaxRangeProvider` of editor/contrib/folding: the active editor's model
//! folds by the extension `FoldingRangeProvider`s registered for it (`editor.defaultFoldingRangeProvider` picks
//! one extension), else by its indentation and markers.

use std::time::Duration;

use config::configuration::Overrides;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::folding::{MAX_FOLDING_REGIONS, SyntaxRange, sanitize_ranges};
use crate::language_features::Candidate;

#[cfg(test)]
mod tests;

/// The `min` of folding.ts `updateDebounceInfo`: the update waits this long after the last trigger.
const UPDATE_DELAY: Duration = Duration::from_millis(200);
/// `MAX_FOLDING_REGIONS_FOR_INDENT_DEFAULT`, the `editor.foldingMaximumRegions` default.
const DEFAULT_MAXIMUM_REGIONS: usize = 5000;

#[derive(Default)]
pub(super) struct FoldingProviders {
    /// Each trigger's number: an update or provider answer of an older one does nothing (`foldingRegionPromise.cancel`).
    generation: u64,
    /// The model id and version of the active editor last triggered for.
    seen: Option<(u32, u32)>,
}

impl Workbench {
    /// folding.ts `onModelChanged` and `onDidChangeModelContent`: another model shown or new content triggers.
    pub(super) fn folding_model_changed(&mut self, cx: &mut Context<Self>) {
        let active = self.editors.active().map(|editor| (editor.model_id, editor.text_model().version_id));
        if active.is_some() && active != self.folding_providers.seen {
            self.folding_providers.seen = active;
            self.trigger_folding_model_changed(cx);
        }
    }

    /// `triggerFoldingModelChanged` of the active editor: its regions are computed again after the update delay.
    pub(crate) fn trigger_folding_model_changed(&mut self, cx: &mut Context<Self>) {
        let Some(model_id) = self.editors.active().map(|editor| editor.model_id) else { return };
        self.folding_providers.generation += 1;
        let generation = self.folding_providers.generation;
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(UPDATE_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.compute_folding_ranges(generation, model_id, cx));
        })
        .detach();
    }

    /// `getRangeProvider` then `SyntaxRangeProvider.compute`: every selected provider's `$provideFoldingRanges`;
    /// with none (or `editor.foldingStrategy` `indentation`) the model folds by indentation.
    fn compute_folding_ranges(&mut self, generation: u64, model_id: u32, cx: &mut Context<Self>) {
        if generation != self.folding_providers.generation {
            return;
        }
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == model_id) else { return };
        let handles = self.folding_range_providers(&editor.document().uri(), &editor.document().language_id);
        if handles.is_empty() {
            let editor = self.editors.active_mut().expect("found above");
            if editor.syntax_folding {
                editor.apply_syntax_folding(None, false);
                self.folding_regions_changed(cx);
            }
            return;
        }
        let (uri, version, line_count) = (editor.document().uri(), editor.text_model().version_id, editor.document().lines.len());
        let calls: Vec<_> = handles.iter().map(|handle| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideFoldingRanges", vec![json!(handle), uri.clone(), json!({})])).collect();
        cx.spawn(async move |workbench, cx| {
            let mut answers = Vec::new();
            for call in calls {
                answers.push(call.await.ok().and_then(Result::ok).unwrap_or(Value::Null));
                // A newer trigger drops the remaining calls, which cancels them on the host.
                if workbench.read_with(cx, |workbench, _| workbench.folding_providers.generation != generation).unwrap_or(true) {
                    return;
                }
            }
            let _ = workbench.update(cx, |workbench, cx| workbench.apply_folding_answers(model_id, version, line_count, answers, cx));
        })
        .detach();
    }

    /// `collectSyntaxRanges`: the ranges of every provider that answered with an array, valid for the model's
    /// lines, ranked by the provider's order; none answering an array falls back to the indentation provider.
    fn apply_folding_answers(&mut self, model_id: u32, version: u32, line_count: usize, answers: Vec<Value>, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == model_id && editor.text_model().version_id == version) else { return };
        let document = editor.document();
        let overrides = Overrides { resource: Some(&document.path), override_identifier: Some(&document.language_id) };
        let settings = self.settings.lock().unwrap();
        let limit = settings.get_value(Some("editor.foldingMaximumRegions"), &overrides).and_then(|value| value.as_u64()).map_or(DEFAULT_MAXIMUM_REGIONS, |limit| (limit as usize).clamp(10, MAX_FOLDING_REGIONS));
        let fold_imports = settings.get_value(Some("editor.foldingImportsByDefault"), &overrides) == Some(json!(true));
        drop(settings);
        let mut ranges: Option<Vec<SyntaxRange>> = None;
        for (rank, answer) in answers.iter().enumerate() {
            let Some(answer) = answer.as_array() else { continue };
            let ranges = ranges.get_or_insert_with(Vec::new);
            for range in answer {
                let (Some(start), Some(end)) = (range["start"].as_u64(), range["end"].as_u64()) else { continue };
                let (start, end) = (start as usize, end as usize);
                if start > 0 && end > start && end <= line_count {
                    ranges.push(SyntaxRange { start, end, rank, kind: range["kind"]["value"].as_str().map(str::to_string) });
                }
            }
        }
        let regions = ranges.map(|ranges| sanitize_ranges(ranges, limit));
        drop(document);
        self.editors.active_mut().expect("found above").apply_syntax_folding(regions, fold_imports);
        self.folding_regions_changed(cx);
    }

    /// `FoldingController.getFoldingRangeProviders` with `DefaultFoldingRangeProvider._selectFoldingRangeProvider`:
    /// the handles of the providers for the model, only the configured extension's when one is set.
    fn folding_range_providers(&self, uri: &Value, language: &str) -> Vec<u64> {
        let settings = self.settings.lock().unwrap();
        let overrides = Overrides { resource: None, override_identifier: Some(language) };
        if settings.get_value(Some("editor.folding"), &overrides) == Some(json!(false)) || settings.get_value(Some("editor.foldingStrategy"), &overrides) == Some(json!("indentation")) {
            return Vec::new();
        }
        let default_provider = settings.get_value(Some("editor.defaultFoldingRangeProvider"), &overrides).and_then(|value| value.as_str().map(str::to_string));
        drop(settings);
        let providers = self.language_features.ordered("$registerFoldingRangeProvider", &Candidate { uri, language });
        let selected = providers.into_iter().filter(|registration| default_provider.as_deref().is_none_or(|id| registration.args.get(2).is_some_and(|extension| extension["value"] == id)));
        selected.map(|registration| registration.handle).collect()
    }

    /// `$emitFoldingRangeEvent`: a provider whose `onDidChangeFoldingRanges` fired folds the active model again.
    pub(crate) fn folding_range_event(&mut self, event_handle: &Value, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let registrations = self.language_features.ordered("$registerFoldingRangeProvider", &Candidate { uri: &uri, language: &language });
        if registrations.iter().any(|registration| registration.args.get(3) == Some(event_handle)) {
            self.trigger_folding_model_changed(cx);
        }
    }

    /// The hidden lines can have changed: the host hears the visible ranges, the window redraws.
    fn folding_regions_changed(&mut self, cx: &mut Context<Self>) {
        self.host.send_visible_ranges(&self.editors);
        cx.notify();
    }
}
