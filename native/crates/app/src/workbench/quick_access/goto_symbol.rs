//! Go to Symbol in Editor (gotoSymbolQuickAccess.ts of editor/contrib/quickAccess and workbench/contrib/codeEditor):
//! the `@` provider of quick access, the document symbols of the active editor filtered by name and container,
//! `@:` grouped by kind; the active pick is revealed and highlighted in the editor.

use gpui::{Context, Task, Window};

use super::Accept;
use super::goto_line::GotoLineState;
use crate::fuzzy_scorer::{piece_to_query, prepare_query, score_fuzzy2};
use crate::outline_model::DocumentSymbol;
use crate::quick_input::{Entry, Item};
use crate::workbench::Workbench;

pub const PREFIX: &str = "@";
const SCOPE_PREFIX: &str = ":";
pub const PREFIX_BY_CATEGORY: &str = "@:";
pub const PLACEHOLDER: &str = "Type the name of a symbol to go to.";
const NO_EDITOR: &str = "To go to a symbol, first open a text editor with symbol information.";
const NO_PROVIDER: &str = "The active text editor does not provide symbol information.";
/// `SymbolTag.Deprecated`.
const DEPRECATED: u64 = 1;

/// The picker's symbols: the model they are of and, once the providers answered, the flat list.
pub struct SymbolPickState {
    pub input: i64,
    model_id: u32,
    symbols: Option<Vec<DocumentSymbol>>,
    _request: Task<()>,
}

/// A symbol pick: where accepting it moves the cursor and the lines its preview highlights.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolTarget {
    pub selection: (usize, usize),
    /// The symbol's range, 1-based.
    pub decoration: crate::outline_model::Range,
}

impl Workbench {
    /// `provideWithTextEditor`: the picks of the active editor's symbols, asked once per picker; a label pick
    /// without an editor or a provider.
    pub(in crate::workbench) fn symbol_picks(&mut self, id: i64, value: &str, window: &Window, cx: &mut Context<Self>) -> Option<(Vec<Entry>, Vec<Accept>, bool)> {
        let Some(editor) = self.editors.active() else { return Some(label_pick(NO_EDITOR)) };
        let model_id = editor.model_id;
        if !self.has_document_symbol_provider(model_id) {
            return Some(label_pick(NO_PROVIDER));
        }
        self.quick_access.goto_line.get_or_insert_with(|| GotoLineState::new(editor));
        if self.quick_access.symbols.as_ref().is_none_or(|state| state.input != id || state.model_id != model_id) {
            let outline = self.outline_model(model_id, cx);
            let request = cx.spawn_in(window, async move |workbench, cx| {
                let symbols = outline.await.map(|outline| outline.as_list_of_document_symbols()).unwrap_or_default();
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.symbols_resolved(id, symbols, window, cx));
            });
            self.quick_access.symbols = Some(SymbolPickState { input: id, model_id, symbols: None, _request: request });
        }
        // Busy until the providers answer.
        let Some(symbols) = self.quick_access.symbols.as_ref().and_then(|state| state.symbols.as_ref()) else { return Some((Vec::new(), Vec::new(), true)) };
        let filter = value[PREFIX.len()..].trim();
        let (entries, accepts) = symbol_entries(symbols, filter);
        if accepts.is_empty() {
            return Some(label_pick(if filter.is_empty() { "No editor symbols" } else { "No matching editor symbols" }));
        }
        Some((entries, accepts.into_iter().map(Accept::Symbol).collect(), false))
    }

    /// The symbols arrived: the picks fill in, and with an empty filter the innermost one around the cursor is
    /// the active pick (`positionToEnclose`).
    fn symbols_resolved(&mut self, id: i64, symbols: Vec<DocumentSymbol>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.quick_access.symbols.as_mut().filter(|state| state.input == id) else { return };
        state.symbols = Some(symbols);
        self.update_quick_access_picks(id, window, cx);
        let Some(input) = self.quick_inputs.get(id) else { return };
        let cursor = self.editors.active().map(|editor| {
            let head = editor.selection().head;
            (head.line + 1, crate::editor::utf16_column(&editor.document().lines[head.line], head.column) + 1)
        });
        let enclosing = match input.input_box.value[PREFIX.len()..].trim().is_empty() {
            true => match self.quick_inputs.owner(id) {
                Some(crate::main_thread_quick_open::Owner::QuickAccess(picks)) => cursor.and_then(|cursor| picks.iter().rposition(|accept| matches!(accept, Accept::Symbol(target) if target.decoration.contains(cursor)))),
                _ => None,
            },
            false => None,
        };
        if let Some(handle) = enclosing {
            self.quick_inputs.get_mut(id).expect("checked above").set_active_items(&[handle as i64]);
        }
        self.flush_quick_input_events(window, cx);
        cx.notify();
    }

    /// `gotoLocation`: the cursor at the symbol's selection start, revealed in the center.
    pub(in crate::workbench) fn go_to_symbol(&mut self, target: &SymbolTarget, in_background: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let position = position_of(editor, target.selection);
        self.go_to_position(position, in_background, window, cx);
    }

    /// `onDidChangeActive`: the active pick's symbol revealed in the center and its lines highlighted.
    pub(in crate::workbench) fn preview_symbol(&mut self, target: &SymbolTarget, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let Some(state) = self.quick_access.goto_line.as_mut().filter(|state| state.previews(editor.model_id)) else { return };
        state.set_highlight(Some((target.decoration.start.0 - 1, target.decoration.end.0 - 1)));
        let position = position_of(editor, target.selection);
        editor.reveal_range(position, position, crate::editor::VerticalReveal::Center, viewport);
        self.editor_scrolled(cx);
    }
}

/// The 1-based UTF-16 position in the editor as a model position, clamped to the text.
pub fn position_of(editor: &crate::editor::OpenEditor, (line, column): (usize, usize)) -> crate::editor::Position {
    let line = (line.max(1) - 1).min(editor.document().lines.len() - 1);
    crate::editor::Position::new(line, crate::editor::char_column(&editor.document().lines[line], column.max(1) - 1))
}

fn label_pick(label: &str) -> (Vec<Entry>, Vec<Accept>, bool) {
    (vec![Entry::Item(Item { handle: 0, label: label.into(), pickable: true, ..Item::default() })], vec![Accept::Nothing], false)
}

struct Pick {
    index: usize,
    kind: u64,
    score: Option<i32>,
    item: Item,
    target: SymbolTarget,
}

/// `doGetSymbolPicks`: the symbols scored on `$(icon) name` (the first word of a multi-word query on the name,
/// the rest on the container), sorted by score or with `:` by kind label then score, under a `symbols (N)`
/// separator or one separator per kind.
pub fn symbol_entries(symbols: &[DocumentSymbol], filter: &str) -> (Vec<Entry>, Vec<SymbolTarget>) {
    let query = prepare_query(filter);
    let filter_by_kind = query.piece.original.starts_with(SCOPE_PREFIX);
    let filter_pos = usize::from(filter_by_kind);
    let (symbol_query, container_query) = match &query.values {
        Some(values) if values.len() > 1 => (piece_to_query(&values[..1]), Some(piece_to_query(&values[1..]))),
        _ => (query.clone(), None),
    };
    let mut picks = Vec::new();
    for (index, symbol) in symbols.iter().enumerate() {
        let name = symbol.name.trim();
        let label = format!("$({}) {name}", crate::symbol_icons::codicon(symbol.kind));
        let icon_offset = label.chars().count() - name.chars().count();
        let container = symbol.container_name.clone().filter(|container| !container.is_empty());
        let (mut score, mut matches, mut container_matches) = (None, Vec::new(), Vec::new());
        if query.piece.original.chars().count() > filter_pos {
            let mut skip_container = false;
            if symbol_query != query {
                let whole = crate::fuzzy_scorer::PreparedQuery { values: None, ..query.clone() };
                (score, matches) = score_fuzzy2(&label, &whole, filter_pos, icon_offset);
                skip_container = score.is_some();
            }
            if score.is_none() {
                (score, matches) = score_fuzzy2(&label, &symbol_query, filter_pos, icon_offset);
                if score.is_none() {
                    continue;
                }
            }
            if let Some(container_query) = container_query.as_ref().filter(|_| !skip_container) {
                let mut container_score = None;
                if let Some(container) = container.as_deref().filter(|_| !container_query.piece.original.is_empty()) {
                    (container_score, container_matches) = score_fuzzy2(container, container_query, 0, 0);
                }
                let Some(container_score) = container_score else { continue };
                score = score.map(|score| score + container_score);
            }
        }
        let deprecated = symbol.tags.contains(&DEPRECATED);
        let item = Item {
            label,
            description: container,
            pickable: true,
            label_highlights: if deprecated { Vec::new() } else { matches },
            description_highlights: if deprecated { Vec::new() } else { container_matches },
            strikethrough: deprecated,
            ..Item::default()
        };
        let target = SymbolTarget { selection: symbol.selection_range.start, decoration: symbol.range };
        picks.push(Pick { index, kind: symbol.kind, score, item, target });
    }
    let by_score = |a: &Pick, b: &Pick| match (a.score, b.score) {
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (a_score, b_score) => b_score.cmp(&a_score).then(a.index.cmp(&b.index)),
    };
    match filter_by_kind {
        true => picks.sort_by(|a, b| crate::command_palette::locale_compare(kind_label(a.kind), kind_label(b.kind)).then_with(|| by_score(a, b))),
        false => picks.sort_by(by_score),
    }
    let mut entries = Vec::new();
    let mut targets = Vec::new();
    if !filter_by_kind && !picks.is_empty() {
        entries.push(Entry::Separator(Some(format!("symbols ({})", picks.len()))));
    }
    let mut kind_start: Option<(usize, u64)> = None;
    for pick in picks {
        if filter_by_kind && kind_start.is_none_or(|(_, kind)| kind != pick.kind) {
            close_kind_separator(&mut entries, kind_start);
            kind_start = Some((entries.len(), pick.kind));
            entries.push(Entry::Separator(None));
        }
        entries.push(Entry::Item(Item { handle: targets.len() as i64, ..pick.item }));
        targets.push(pick.target);
    }
    close_kind_separator(&mut entries, kind_start);
    (entries, targets)
}

/// `updateLastSeparatorLabel`: the kind's separator gets the count of picks after it.
fn close_kind_separator(entries: &mut [Entry], kind_start: Option<(usize, u64)>) {
    let Some((at, kind)) = kind_start else { return };
    let count = entries[at + 1..].len();
    entries[at] = Entry::Separator(Some(kind_label(kind).replace("{0}", &count.to_string())));
}

/// `NLS_SYMBOL_KIND_CACHE`, `FALLBACK_NLS_SYMBOL_KIND` for the kinds it lacks.
fn kind_label(kind: u64) -> &'static str {
    match kind {
        5 => "methods ({0})",
        11 => "functions ({0})",
        8 => "constructors ({0})",
        12 => "variables ({0})",
        4 => "classes ({0})",
        22 => "structs ({0})",
        23 => "events ({0})",
        24 => "operators ({0})",
        10 => "interfaces ({0})",
        2 => "namespaces ({0})",
        3 => "packages ({0})",
        25 => "type parameters ({0})",
        1 => "modules ({0})",
        6 => "properties ({0})",
        9 => "enumerations ({0})",
        21 => "enumeration members ({0})",
        14 => "strings ({0})",
        0 => "files ({0})",
        17 => "arrays ({0})",
        15 => "numbers ({0})",
        16 => "booleans ({0})",
        18 => "objects ({0})",
        19 => "keys ({0})",
        7 => "fields ({0})",
        13 => "constants ({0})",
        _ => "properties ({0})",
    }
}

#[cfg(test)]
mod tests;
