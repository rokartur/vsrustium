//! workbench/contrib/folding with the `SyntaxRangeProvider` of editor/contrib/folding: the active editor's model
//! folds by the extension `FoldingRangeProvider`s registered for it (`editor.defaultFoldingRangeProvider` picks
//! one extension), else by its indentation and markers.

use std::time::Duration;

use config::configuration::Overrides;
use gpui::{AppContext, Context};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::folding::{FoldingRegions, MAX_FOLDING_REGIONS, SyntaxRange, compute_ranges, sanitize_ranges};
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
    pub(super) fn register_folding_providers(&mut self) {
        self.commands.register("_executeFoldingRangeProvider", |workbench, args, _, cx| workbench.execute_folding_range_provider(args.first().cloned().unwrap_or_default(), cx));
    }

    /// folding.ts `_executeFoldingRangeProvider`: the ranges of any open model as the folding controller would compute
    /// them, none with `editor.folding` off.
    fn execute_folding_range_provider(&mut self, uri: Value, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&uri).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else { return Outcome::error("Illegal argument") };
        let document = editor.document();
        let overrides = Overrides { resource: Some(&document.path), override_identifier: Some(&document.language_id) };
        if self.settings.lock().unwrap().get_value(Some("editor.folding"), &overrides) == Some(json!(false)) {
            return Outcome::Ready(Ok(json!([])));
        }
        let limit = self.folding_limit(&overrides);
        let indentation = compute_ranges(&document.lines, document.indentation.tab_size, document.off_side, document.folding_markers.as_deref());
        let line_count = document.lines.len();
        let handles = self.folding_range_providers(&document.uri(), &document.language_id);
        let calls: Vec<_> = handles.iter().map(|handle| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideFoldingRanges", vec![json!(handle), uri.clone(), json!({})])).collect();
        drop(document);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.background_spawn(async move {
            let mut answers = Vec::new();
            for call in calls {
                answers.push(call.await.ok().and_then(Result::ok).unwrap_or(Value::Null));
            }
            let regions = syntax_regions(&answers, line_count, limit).unwrap_or(indentation);
            let ranges = regions.ranges().iter().map(|range| match &range.kind {
                Some(kind) => json!({ "start": range.start, "end": range.end, "kind": { "value": kind } }),
                None => json!({ "start": range.start, "end": range.end }),
            });
            let _ = sender.send(Ok(Value::Array(ranges.collect())));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `editor.foldingMaximumRegions`, clamped as `FoldingController` does.
    fn folding_limit(&self, overrides: &Overrides) -> usize {
        let settings = self.settings.lock().unwrap();
        settings.get_value(Some("editor.foldingMaximumRegions"), overrides).and_then(|value| value.as_u64()).map_or(DEFAULT_MAXIMUM_REGIONS, |limit| (limit as usize).clamp(10, MAX_FOLDING_REGIONS))
    }

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
        let limit = self.folding_limit(&overrides);
        let fold_imports = self.settings.lock().unwrap().get_value(Some("editor.foldingImportsByDefault"), &overrides) == Some(json!(true));
        let regions = syntax_regions(&answers, line_count, limit);
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

/// `collectSyntaxRanges` and `sanitizeAndMerge`: the ranges of every provider that answered with an array, valid for
/// the model's lines, ranked by the provider's order; `None` when none answered an array.
fn syntax_regions(answers: &[Value], line_count: usize, limit: usize) -> Option<FoldingRegions> {
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
    ranges.map(|ranges| sanitize_ranges(ranges, limit))
}
