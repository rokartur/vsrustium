//! Go to Symbol in Workspace (symbolsQuickAccess.ts, search.ts `getWorkspaceSymbols`): the `#` provider of quick
//! access, the symbols every workspace symbol provider gives for the first word of the filter, 200ms after the last
//! keystroke, scored on the name and the container.

use std::time::Duration;

use gpui::{Context, Task, Window};
use serde_json::{Value, json};

use super::Accept;
use crate::fuzzy_scorer::{PreparedQuery, piece_to_query, prepare_query, score_fuzzy2};
use crate::outline_model::Range;
use crate::quick_input::{Entry, Item};
use crate::references_model::uri_key;
use crate::workbench::Workbench;

pub const PREFIX: &str = "#";
pub const PLACEHOLDER: &str = "Type the name of a symbol to open.";
pub const REGISTER: &str = "$registerNavigateTypeSupport";
const NO_RESULTS: &str = "No matching workspace symbols";
/// `SymbolsQuickAccessProvider.TYPING_SEARCH_DELAY`.
const TYPING_SEARCH_DELAY: Duration = Duration::from_millis(200);
/// `SymbolTag.Deprecated`.
const DEPRECATED: u64 = 1;

/// The picker's search: the query the providers were asked for and, once they all answered, their symbols.
pub struct WorkspaceSymbolState {
    pub input: i64,
    query: String,
    symbols: Option<Vec<WorkspaceSymbol>>,
    _request: Task<()>,
}

/// `WorkspaceSymbolItem`: a symbol and the provider that gave it.
#[derive(Clone, Debug)]
pub struct WorkspaceSymbol {
    handle: u64,
    supports_resolve: bool,
    /// `IWorkspaceSymbolDto`, sent back to resolve it.
    dto: Value,
    name: String,
    kind: u64,
    container: Option<String>,
    uri: String,
    range: Range,
}

impl WorkspaceSymbol {
    fn from_dto(handle: u64, supports_resolve: bool, dto: &Value) -> Option<Self> {
        let location = &dto["location"];
        Some(WorkspaceSymbol {
            handle,
            supports_resolve,
            name: dto["name"].as_str()?.to_string(),
            kind: dto["kind"].as_u64().unwrap_or(0),
            container: dto["containerName"].as_str().filter(|name| !name.is_empty()).map(String::from),
            uri: uri_key(&location["uri"])?,
            range: Range::from_dto(&location["range"])?,
            dto: dto.clone(),
        })
    }

    fn deprecated(&self) -> bool {
        self.dto["tags"].as_array().is_some_and(|tags| tags.contains(&json!(DEPRECATED)))
    }
}

impl Workbench {
    /// `_getPicks`: the symbols of the providers for the filter's first word, asked again when that word changes.
    pub(in crate::workbench) fn workspace_symbol_picks(&mut self, id: i64, value: &str, window: &Window, cx: &mut Context<Self>) -> Option<(Vec<Entry>, Vec<Accept>, bool)> {
        let query = prepare_query(value[PREFIX.len()..].trim());
        let symbol_query = symbol_query(&query);
        if self.quick_access.workspace_symbols.as_ref().is_none_or(|state| state.input != id || state.query != symbol_query.piece.original) {
            let request = self.search_workspace_symbols(id, symbol_query.piece.original.clone(), window, cx);
            self.quick_access.workspace_symbols = Some(WorkspaceSymbolState { input: id, query: symbol_query.piece.original, symbols: None, _request: request });
        }
        let Some(symbols) = self.quick_access.workspace_symbols.as_ref().and_then(|state| state.symbols.as_ref()) else { return Some((Vec::new(), Vec::new(), true)) };
        let (entries, picks) = workspace_symbol_entries(symbols, &query, |uri| self.workspace_symbol_path(uri));
        if picks.is_empty() {
            let no_results = Item { handle: 0, label: NO_RESULTS.into(), pickable: true, ..Item::default() };
            return Some((vec![Entry::Item(no_results)], vec![Accept::Nothing], false));
        }
        Some((entries, picks.into_iter().map(Accept::WorkspaceSymbol).collect(), false))
    }

    /// `getWorkspaceSymbols` after the typing delay: every provider asked, each one's previous result released
    /// (`$releaseWorkspaceSymbols`), a failing one giving nothing, then the duplicates dropped.
    fn search_workspace_symbols(&mut self, id: i64, query: String, window: &Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(TYPING_SEARCH_DELAY).await;
            let Ok(calls) = workbench.update(cx, |workbench, _| {
                let providers: Vec<(u64, bool)> = workbench.language_features.all(REGISTER).map(|registration| (registration.handle, registration.args.get(1) == Some(&json!(true)))).collect();
                providers
                    .into_iter()
                    .map(|(handle, resolve)| (handle, resolve, workbench.host.call_cancellable("ExtHostLanguageFeatures", "$provideWorkspaceSymbols", vec![json!(handle), json!(query)])))
                    .collect::<Vec<_>>()
            }) else {
                return;
            };
            let mut symbols = Vec::new();
            let mut results = Vec::new();
            for (handle, supports_resolve, call) in calls {
                let Ok(Ok(answer)) = call.await else { continue };
                if let Some(cache_id) = answer["cacheId"].as_u64() {
                    results.push((handle, cache_id));
                }
                symbols.extend(answer["symbols"].as_array().into_iter().flatten().filter_map(|dto| WorkspaceSymbol::from_dto(handle, supports_resolve, dto)));
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                for (handle, cache_id) in results {
                    if let Some(previous) = workbench.quick_access.workspace_symbol_results.insert(handle, cache_id) {
                        workbench.host.send("ExtHostLanguageFeatures", "$releaseWorkspaceSymbols", vec![json!(handle), json!(previous)]);
                    }
                }
                let Some(state) = workbench.quick_access.workspace_symbols.as_mut().filter(|state| state.input == id) else { return };
                state.symbols = Some(deduplicate(symbols));
                workbench.update_quick_access_picks(id, window, cx);
            });
        })
    }

    /// `labelService.getUriLabel(uri, { relative: true })`.
    fn workspace_symbol_path(&self, uri: &str) -> String {
        match uri.strip_prefix("file://") {
            Some(path) => self.peek_folder_label(&crate::uri::percent_decode(path)),
            None => uri.to_string(),
        }
    }

    /// `openSymbol`: the symbol resolved when its provider can, then a web link opened outside and anything else in
    /// the editor at the start of its range, pinned unless `workbench.editor.enablePreviewFromQuickOpen`.
    pub(in crate::workbench) fn open_workspace_symbol(&mut self, symbol: WorkspaceSymbol, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let resolve = symbol.supports_resolve.then(|| self.host.call_cancellable("ExtHostLanguageFeatures", "$resolveWorkspaceSymbol", vec![json!(symbol.handle), symbol.dto.clone()]));
        cx.spawn_in(window, async move |workbench, cx| {
            let mut location = symbol.dto["location"].clone();
            if let Some(resolve) = resolve
                && let Ok(Ok(resolved)) = resolve.await
                && resolved.is_object()
            {
                location = resolved["location"].clone();
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let scheme = location["uri"]["scheme"].as_str().unwrap_or_default();
                if scheme == "http" || scheme == "https" {
                    if let Ok(uri) = crate::uri::Uri::from_json(&location["uri"]) {
                        let options = super::super::opener::OpenOptions { contributed_openers: super::super::opener::ContributedOpeners::Any, ..Default::default() };
                        drop(workbench.open_link(super::super::opener::Target::Uri(uri), options, window, cx));
                    }
                    return;
                }
                let start = &location["range"];
                let selection = json!({ "startLineNumber": start["startLineNumber"], "startColumn": start["startColumn"], "endLineNumber": start["startLineNumber"], "endColumn": start["startColumn"] });
                let pinned = in_background || workbench.setting("workbench.editor.enablePreviewFromQuickOpen") != true || workbench.setting("workbench.editor.enablePreview") == false;
                let _ = workbench.try_show_text_document(&location["uri"], &json!({ "selection": selection, "pinned": pinned, "preserveFocus": in_background }), window, cx);
            });
        })
        .detach();
    }
}

/// The query the providers get: the first word of a multi-word filter, the rest matches the container.
fn symbol_query(query: &PreparedQuery) -> PreparedQuery {
    match &query.values {
        Some(values) if values.len() > 1 => piece_to_query(&values[..1]),
        _ => query.clone(),
    }
}

/// `getWorkspaceSymbols`' `compareItems` and `groupBy`: of the symbols with the same name, kind, resource,
/// intersecting range and container, the first one (a resolving provider's first).
fn deduplicate(mut symbols: Vec<WorkspaceSymbol>) -> Vec<WorkspaceSymbol> {
    let compare = |a: &WorkspaceSymbol, b: &WorkspaceSymbol| {
        let intersect = !(a.range.end <= b.range.start || b.range.end <= a.range.start);
        a.name
            .cmp(&b.name)
            .then(a.kind.cmp(&b.kind))
            .then(a.uri.cmp(&b.uri))
            .then(if intersect { std::cmp::Ordering::Equal } else { a.range.cmp(&b.range) })
            .then(a.container.as_deref().unwrap_or_default().cmp(b.container.as_deref().unwrap_or_default()))
    };
    symbols.sort_by(compare);
    symbols.dedup_by(|later, first| compare(first, later).is_eq());
    symbols
}

/// `doGetSymbolPicks`: the symbols scored on the name (the whole query first, then its first word with the rest on
/// the container), described by their container and path, by score, then name, then kind.
pub fn workspace_symbol_entries(symbols: &[WorkspaceSymbol], query: &PreparedQuery, path_of: impl Fn(&str) -> String) -> (Vec<Entry>, Vec<WorkspaceSymbol>) {
    let symbol_query = symbol_query(query);
    let container_query = query.values.as_ref().filter(|values| values.len() > 1).map(|values| piece_to_query(&values[1..]));
    let mut picks = Vec::new();
    for symbol in symbols {
        let (mut score, mut matches) = (None, Vec::new());
        let mut skip_container = false;
        if !symbol_query.piece.original.is_empty() {
            if symbol_query != *query {
                (score, matches) = score_fuzzy2(&symbol.name, &PreparedQuery { values: None, ..query.clone() }, 0, 0);
                skip_container = score.is_some();
            }
            if score.is_none() {
                (score, matches) = score_fuzzy2(&symbol.name, &symbol_query, 0, 0);
                if score.is_none() {
                    continue;
                }
            }
        }
        let path = path_of(&symbol.uri);
        let description = match &symbol.container {
            Some(container) => format!("{container} \u{2022} {path}"),
            None => path,
        };
        let mut container_matches = Vec::new();
        if let Some(container_query) = container_query.as_ref().filter(|query| !skip_container && !query.piece.original.is_empty()) {
            let container_score;
            (container_score, container_matches) = score_fuzzy2(&description, container_query, 0, 0);
            let Some(container_score) = container_score else { continue };
            score = score.map(|score| score + container_score);
        }
        let deprecated = symbol.deprecated();
        let item = Item {
            label: symbol.name.clone(),
            description: Some(description),
            icon: Some(crate::symbol_icons::codicon(symbol.kind).to_string()),
            pickable: true,
            label_highlights: if deprecated { Vec::new() } else { matches },
            description_highlights: if deprecated { Vec::new() } else { container_matches },
            strikethrough: deprecated,
            ..Item::default()
        };
        picks.push((score, item, symbol.clone()));
    }
    picks.sort_by(|(a_score, _, a), (b_score, _, b)| {
        let by_score = match (a_score, b_score) {
            (Some(a_score), Some(b_score)) => b_score.cmp(a_score),
            _ => std::cmp::Ordering::Equal,
        };
        by_score
            .then_with(|| crate::command_palette::locale_compare(&a.name.to_lowercase(), &b.name.to_lowercase()))
            .then_with(|| crate::command_palette::locale_compare(crate::symbol_icons::codicon(a.kind), crate::symbol_icons::codicon(b.kind)))
    });
    let mut entries = Vec::new();
    let mut targets = Vec::new();
    for (_, item, symbol) in picks {
        entries.push(Entry::Item(Item { handle: targets.len() as i64, ..item }));
        targets.push(symbol);
    }
    (entries, targets)
}

#[cfg(test)]
mod tests;
