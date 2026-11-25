//! outlineModel.ts `OutlineModelService` and documentSymbols.ts: the document symbols of a model from every
//! `DocumentSymbolProvider`, cached per model version, and the `_executeDocumentSymbolProvider` API command.

use gpui::{Context, Task};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::language_features::Candidate;
use crate::outline_model::{DocumentSymbol, OutlineModel, Range};

pub(super) const REGISTER: &str = "$registerDocumentSymbolProvider";

/// `LRUCache<string, CacheEntry>(15, 0.7)`: the model id, the version the outline was computed for and the outline,
/// the last used last.
#[derive(Default)]
pub(super) struct DocumentSymbols {
    cache: Vec<(u32, u32, OutlineModel)>,
}

const CACHE_LIMIT: usize = 15;
/// `LRUCache.checkTrim`: past the limit it keeps `Math.round(15 * 0.7)`.
const CACHE_TRIM: usize = 11;

impl Workbench {
    pub(super) fn register_document_symbol_commands(&mut self) {
        self.commands.register("_executeDocumentSymbolProvider", |workbench, args, _, cx| {
            let Some((scheme, path)) = args.first().and_then(|uri| super::host_requests::uri_parts(uri).ok()) else { return Outcome::error("Illegal argument") };
            let Some(model_id) = workbench.editors.editors.iter().chain(&workbench.editors.models).find(|editor| editor.document().is(scheme, &path)).map(|editor| editor.model_id) else {
                return Outcome::error("Illegal argument");
            };
            let outline = workbench.outline_model(model_id, cx);
            let (sender, receiver) = futures::channel::oneshot::channel();
            cx.spawn(async move |_, _| {
                let symbols = outline.await.map(|outline| outline.top_level_symbols().into_iter().map(symbol_dto).collect()).unwrap_or_default();
                let _ = sender.send(Ok(Value::Array(symbols)));
            })
            .detach();
            Outcome::Pending(receiver)
        });
    }

    /// `documentSymbolProvider.has(model)`.
    pub(super) fn has_document_symbol_provider(&self, model_id: u32) -> bool {
        let Some(editor) = self.editors.editors.iter().chain(&self.editors.models).find(|editor| editor.model_id == model_id) else { return false };
        let uri = editor.document().uri();
        !self.language_features.ordered(REGISTER, &Candidate { uri: &uri, language: &editor.document().language_id }).is_empty()
    }

    /// `OutlineModelService.getOrCreate`: the cached model of this version, else every provider asked in score order
    /// (a failing one gives no symbols), grouped by provider under its display name. `None` for a closed model.
    pub(super) fn outline_model(&mut self, model_id: u32, cx: &mut Context<Self>) -> Task<Option<OutlineModel>> {
        let Some(editor) = self.editors.editors.iter().chain(&self.editors.models).find(|editor| editor.model_id == model_id) else { return Task::ready(None) };
        let version = editor.text_model().version_id;
        if let Some(index) = self.document_symbols.cache.iter().position(|(id, cached, _)| *id == model_id && *cached == version) {
            let entry = self.document_symbols.cache.remove(index);
            let outline = entry.2.clone();
            self.document_symbols.cache.push(entry);
            return Task::ready(Some(outline));
        }
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        let mut calls = Vec::new();
        for registration in self.language_features.ordered(REGISTER, &candidate) {
            let label = registration.args.get(2).and_then(Value::as_str).unwrap_or("Unknown Outline Provider").to_string();
            calls.push((label, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentSymbols", vec![json!(registration.handle), uri.clone()])));
        }
        cx.spawn(async move |workbench, cx| {
            let mut answers = Vec::new();
            for (label, call) in calls {
                let symbols = match call.await {
                    Ok(Ok(Value::Array(symbols))) => symbols.iter().filter_map(DocumentSymbol::from_dto).collect(),
                    _ => Vec::new(),
                };
                answers.push((label, symbols));
            }
            let outline = OutlineModel::new(answers);
            let _ = workbench.update(cx, |workbench, _| {
                let cache = &mut workbench.document_symbols.cache;
                cache.retain(|(id, ..)| *id != model_id);
                cache.push((model_id, version, outline.clone()));
                if cache.len() > CACHE_LIMIT {
                    cache.drain(..cache.len() - CACHE_TRIM);
                }
            });
            Some(outline)
        })
    }
}

fn range_dto(range: Range) -> Value {
    json!({ "startLineNumber": range.start.0, "startColumn": range.start.1, "endLineNumber": range.end.0, "endColumn": range.end.1 })
}

/// `languages.DocumentSymbol` as the API command answers it.
fn symbol_dto(symbol: &DocumentSymbol) -> Value {
    let mut dto = json!({
        "name": symbol.name,
        "detail": symbol.detail,
        "kind": symbol.kind,
        "tags": symbol.tags,
        "range": range_dto(symbol.range),
        "selectionRange": range_dto(symbol.selection_range),
        "children": symbol.children.iter().map(symbol_dto).collect::<Vec<_>>(),
    });
    if let Some(container) = &symbol.container_name {
        dto["containerName"] = json!(container);
    }
    dto
}
