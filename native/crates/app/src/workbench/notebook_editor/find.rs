//! Notebook find (notebookFindWidget.ts, findModel.ts and notebookFindReplaceWidget.ts): one find and replace widget
//! over the cells of the active notebook editor. Cell sources are searched here; rendered markdown and outputs by the
//! back layer webview's preloads (`find`, answered with `didFind`), whose matches follow the cell's own.

use std::collections::HashSet;
use std::ops::Range;

use gpui::{BoxShadow, ClickEvent, Context, Div, FocusHandle, MouseButton, MouseDownEvent, Window, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::super::Workbench;
use super::super::list_scroll::ListId;
use super::super::input_box_view::{InputId, InputLook, InputView};
use super::overview_ruler::RulerDecoration;
use super::{CellReveal, parse_cell_id};
use crate::editor::find::controller::{Seed, selection_search_string};
use crate::editor::find::model::search_params;
use crate::editor::find::replace_pattern::{ReplacePattern, parse_replace_string};
use crate::editor::find::search::{FindMatch, find_matches};
use crate::editor::find::state::{FindState, MATCHES_LIMIT};
use crate::editor::{Position, Selection};
use crate::input_box::InputBox;
use crate::notebook::{CODE, MARKUP, layout};

/// notebookFindReplaceWidget.ts `NOTEBOOK_FIND_WIDGET_INITIAL_WIDTH` and its horizontal padding.
const WIDGET_WIDTH: f32 = 419.;
const WIDGET_PADDING: f32 = 4.;
/// The back layer's find owner (`notebookEditor.getId()`).
const OWNER_ID: &str = "notebook-find";

/// `NotebookFindFilters`, all on by default (`notebook.find.filters`).
#[derive(Clone, Copy)]
struct Filters {
    markup_source: bool,
    markup_preview: bool,
    code_source: bool,
    code_output: bool,
}

/// `CellFindMatchModel`: a cell's source matches, then its rendered markdown or output matches by webview index.
struct CellMatches {
    handle: u64,
    index: usize,
    uri: Value,
    content: Vec<FindMatch>,
    webview: Vec<u64>,
}

impl CellMatches {
    fn len(&self) -> usize {
        self.content.len() + self.webview.len()
    }
}

pub(in crate::workbench) struct NotebookFind {
    pub(in crate::workbench) state: FindState,
    filters: Filters,
    /// The notebook editor (tab model id) the widget shows over.
    tab: Option<u32>,
    pub(in crate::workbench) find_input: InputBox,
    pub(in crate::workbench) find_view: InputView,
    pub(in crate::workbench) find_focus: FocusHandle,
    pub(in crate::workbench) replace_input: InputBox,
    pub(in crate::workbench) replace_view: InputView,
    pub(in crate::workbench) replace_focus: FocusHandle,
    matches: Vec<CellMatches>,
    /// `_currentMatch`, over all cells.
    current: Option<usize>,
    /// The source matches waiting for the webview's `didFind`.
    pending: Option<Vec<CellMatches>>,
    /// The notebook's and its cells' versions the matches were found in.
    searched: u64,
    /// Markup cells find put in edit mode (`editStateSource` `find`).
    editing: HashSet<u64>,
    focus_to_return: Option<FocusHandle>,
}

impl NotebookFind {
    pub(in crate::workbench) fn new(cx: &mut gpui::App) -> Self {
        NotebookFind {
            state: FindState::default(),
            filters: Filters { markup_source: true, markup_preview: true, code_source: true, code_output: true },
            tab: None,
            find_input: InputBox::new(String::new()),
            find_view: InputView::default(),
            find_focus: cx.focus_handle(),
            replace_input: InputBox::new(String::new()),
            replace_view: InputView::default(),
            replace_focus: cx.focus_handle(),
            matches: Vec::new(),
            current: None,
            pending: None,
            searched: 0,
            editing: HashSet::new(),
            focus_to_return: None,
        }
    }

    fn total(&self) -> usize {
        self.matches.iter().map(CellMatches::len).sum()
    }

    /// The cell and the match in it of `_currentMatch` (`PrefixSumComputer.getIndexOf`).
    fn locate(&self, index: usize) -> Option<(usize, usize)> {
        let mut before = 0;
        for (cell, matches) in self.matches.iter().enumerate() {
            if index < before + matches.len() {
                return Some((cell, index - before));
            }
            before += matches.len();
        }
        None
    }

    /// The matches before the cell at `cell` of the matches (`_matchesCountBeforeIndex`).
    fn count_before(matches: &[CellMatches], cell: usize) -> usize {
        matches[..cell].iter().map(CellMatches::len).sum()
    }
}

impl Workbench {
    pub(in crate::workbench) fn register_notebook_find_commands(&mut self) {
        self.commands.register("notebook.find", |workbench, _, window, cx| {
            workbench.show_notebook_find(false, window, cx);
            crate::commands::Outcome::done()
        });
        self.commands.register("notebook.hideFind", |workbench, _, window, cx| {
            workbench.hide_notebook_find(window, cx);
            crate::commands::Outcome::done()
        });
        for (id, previous) in [("notebook.findNext.fromWidget", false), ("notebook.findPrevious.fromWidget", true)] {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.notebook_find_move(previous, window, cx);
                crate::commands::Outcome::done()
            });
        }
        for key in ["markupSource", "markupPreview", "codeSource", "codeOutput"] {
            self.commands.register(format!("_notebook.find.filter.{key}"), move |workbench, _, _, cx| {
                let filters = &mut workbench.notebook_views.find.filters;
                let filter = match key {
                    "markupSource" => &mut filters.markup_source,
                    "markupPreview" => &mut filters.markup_preview,
                    "codeSource" => &mut filters.code_source,
                    _ => &mut filters.code_output,
                };
                *filter = !*filter;
                workbench.notebook_find_research(cx);
                crate::commands::Outcome::done()
            });
        }
    }

    /// Whether `actions.find` and its kin act on the notebook find (notebookFind.ts `StartFindAction.addImplementation`):
    /// the active editor is a notebook editor's cell or its list.
    pub(in crate::workbench) fn notebook_find_target(&self) -> bool {
        self.active_notebook_tab().is_some()
    }

    /// `NotebookFindContrib.show` and `replace`: seeded from the focused cell editor's selection or word.
    pub(in crate::workbench) fn show_notebook_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tab, _)) = self.active_notebook_tab() else { return };
        let seed = match self.editor_focus.is_focused(window) {
            true => self.editors.active().and_then(|editor| selection_search_string(editor, Seed::Single, false)),
            false => None,
        };
        let find = &mut self.notebook_views.find;
        if !find.state.is_revealed {
            find.focus_to_return = window.focused(cx);
        }
        find.tab = Some(tab);
        find.state.is_revealed = true;
        if let Some(seed) = seed {
            find.state.search_string = seed.clone();
            find.find_input.set_value(seed);
        }
        let in_interactive = self.active_interactive().is_some();
        let find = &mut self.notebook_views.find;
        if replace && !in_interactive {
            find.state.is_replace_revealed = true;
        }
        let (focus, id) = match replace && !in_interactive {
            true => (find.replace_focus.clone(), InputId::NotebookReplace),
            false => (find.find_focus.clone(), InputId::NotebookFind),
        };
        match id {
            InputId::NotebookReplace => find.replace_input.select_all(),
            _ => find.find_input.select_all(),
        }
        window.focus(&focus, cx);
        self.notebook_find_research(cx);
        cx.notify();
    }

    /// `NotebookFindWidget.hide`: the matches go, markup cells find opened go back to their preview, and the focus
    /// returns to where it was.
    pub(in crate::workbench) fn hide_notebook_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.notebook_views.find;
        if !find.state.is_revealed {
            return;
        }
        find.state.is_revealed = false;
        find.matches.clear();
        find.current = None;
        find.pending = None;
        let tab = find.tab.take();
        let editing = std::mem::take(&mut find.editing);
        let focus = find.focus_to_return.take();
        if let Some(view) = tab.and_then(|tab| self.notebook_views.views.get_mut(&tab)) {
            view.editing.retain(|handle| !editing.contains(handle));
            if let Some(layer) = view.webview.as_mut() {
                layer.post(json!({ "type": "findStop", "ownerID": OWNER_ID }));
            }
        }
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn notebook_find_visible(&self) -> bool {
        self.notebook_views.find.state.is_revealed && self.notebook_views.find.tab.is_some() && self.notebook_views.find.tab == self.active_notebook_tab().map(|(tab, _)| tab)
    }

    /// The find inputs have the keyboard: the widget lies inside the notebook editor (`notebookEditorFocused`).
    pub(in crate::workbench) fn notebook_find_focused(&self, window: &Window) -> bool {
        let find = &self.notebook_views.find;
        find.find_focus.is_focused(window) || find.replace_focus.is_focused(window)
    }

    /// The input's text changed: the state follows and the search runs again (`onInputChanged`).
    pub(in crate::workbench) fn edit_notebook_find_input(&mut self, id: InputId, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        let find = &mut self.notebook_views.find;
        match id {
            InputId::NotebookReplace => {
                edit(&mut find.replace_input);
                find.state.replace_string = find.replace_input.value.clone();
            }
            _ => {
                let before = find.find_input.value.clone();
                edit(&mut find.find_input);
                if find.find_input.value != before {
                    find.state.search_string = find.find_input.value.clone();
                    self.notebook_find_research(cx);
                }
            }
        }
        cx.notify();
    }

    /// Enter finds the next match, Shift+Enter the previous one; Enter in the replace input replaces
    /// (`_onFindInputKeyDown`, `_onReplaceInputKeyDown`). Escape hides the widget (`onkeyup`).
    pub(in crate::workbench) fn notebook_find_input_key(&mut self, id: InputId, keystroke: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        match (keystroke.key.as_str(), id) {
            ("enter", InputId::NotebookFind) if !modifiers.platform && !modifiers.alt && !modifiers.control => self.notebook_find_move(modifiers.shift, window, cx),
            ("enter", InputId::NotebookReplace) if !modifiers.modified() => self.notebook_find_replace_one(window, cx),
            ("escape", _) if !modifiers.modified() => self.hide_notebook_find(window, cx),
            ("tab", InputId::NotebookFind) if !modifiers.shift && self.notebook_views.find.state.is_replace_revealed => window.focus(&self.notebook_views.find.replace_focus.clone(), cx),
            ("tab", InputId::NotebookReplace) if modifiers.shift => window.focus(&self.notebook_views.find.find_focus.clone(), cx),
            _ => return false,
        }
        true
    }

    /// A version stamp of the notebook the widget searches: its cells, their texts and outputs.
    fn notebook_find_version(&self, notebook: &str) -> u64 {
        let Some(model) = self.host.notebooks.model(notebook) else { return 0 };
        let mut version = model.version_id.wrapping_mul(31).wrapping_add(model.cells.len() as u64);
        for cell in &model.cells {
            let text = self.cell_model(&cell.uri).map_or(0, |editor| editor.text_model().version_id);
            version = version.wrapping_mul(31).wrapping_add(u64::from(text));
        }
        version
    }

    /// Runs each render: an edit of the notebook searches again (findModel.ts `_registerModelListener`).
    pub(in crate::workbench) fn notebook_find_follow_edits(&mut self, cx: &mut Context<Self>) {
        if !self.notebook_find_visible() {
            return;
        }
        let Some((_, notebook)) = self.active_notebook_tab() else { return };
        if self.notebook_find_version(&notebook) != self.notebook_views.find.searched {
            self.notebook_find_research(cx);
        }
    }

    /// `_research` with `notebookEditorWidget.find` and `NotebookViewModel.find`: the source matches now, the webview's
    /// once `didFind` arrives.
    pub(in crate::workbench) fn notebook_find_research(&mut self, cx: &mut Context<Self>) {
        let Some((tab, notebook)) = self.active_notebook_tab() else { return };
        self.notebook_views.find.searched = self.notebook_find_version(&notebook);
        let find = &self.notebook_views.find;
        if !find.state.is_revealed || find.state.search_string.is_empty() {
            self.notebook_find_set(tab, Vec::new(), cx);
            if let Some(layer) = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()) {
                layer.post(json!({ "type": "findStop", "ownerID": OWNER_ID }));
            }
            return;
        }
        let Some(model) = self.host.notebooks.model(&notebook) else { return };
        let filters = find.filters;
        let params = search_params(&find.state);
        let editing = self.notebook_views.views.get(&tab).map(|view| view.editing.clone()).unwrap_or_default();
        let mut matches = Vec::new();
        for (index, cell) in model.cells.iter().enumerate() {
            let Some(editor) = self.cell_model(&cell.uri) else { continue };
            // notebookViewModelImpl.ts `find`: a markup preview's source counts only without the rendered markdown.
            let included = match cell.kind {
                CODE => filters.code_source,
                _ if editing.contains(&cell.handle) => filters.markup_source,
                _ => !filters.markup_preview && filters.markup_source,
            };
            if !included {
                continue;
            }
            let lines = &editor.document().lines;
            let end = Position::new(lines.len() - 1, lines[lines.len() - 1].chars().count());
            let content = find_matches(lines, &params, Position::default()..end, true, MATCHES_LIMIT);
            if !content.is_empty() {
                matches.push(CellMatches { handle: cell.handle, index, uri: cell.uri.clone(), content, webview: Vec::new() });
            }
        }
        let layer = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()).filter(|layer| layer.initialized);
        match layer {
            Some(layer) if filters.markup_preview || filters.code_output => {
                let state = &self.notebook_views.find.state;
                let options = json!({ "caseSensitive": state.match_case, "wholeWord": state.whole_word, "includeMarkup": filters.markup_preview, "includeOutput": filters.code_output, "shouldGetSearchPreviewInfo": false, "ownerID": OWNER_ID, "findIds": [] });
                layer.post(json!({ "type": "find", "query": state.search_string, "options": options }));
                self.notebook_views.find.pending = Some(matches);
            }
            layer => {
                if let Some(layer) = layer {
                    layer.post(json!({ "type": "findStop", "ownerID": OWNER_ID }));
                }
                self.notebook_find_set(tab, matches, cx);
            }
        }
    }

    /// The webview's `didFind`: its matches join the source matches of their cells, in cell order.
    pub(in crate::workbench) fn notebook_find_did_find(&mut self, tab: u32, webview_matches: &[Value], cx: &mut Context<Self>) {
        let Some(mut matches) = self.notebook_views.find.pending.take() else { return };
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return };
        let editing = self.notebook_views.views.get(&tab).map(|view| view.editing.clone()).unwrap_or_default();
        let filters = self.notebook_views.find.filters;
        for found in webview_matches {
            let Some(handle) = found["cellId"].as_str().and_then(parse_cell_id) else { continue };
            let Some(index) = model.index_of_handle(handle) else { continue };
            let wanted = match found["type"].as_str() {
                Some("preview") => filters.markup_preview && !editing.contains(&handle),
                _ => filters.code_output,
            };
            if !wanted {
                continue;
            }
            let webview_index = found["index"].as_u64().unwrap_or_default();
            match matches.iter_mut().find(|cell| cell.handle == handle) {
                Some(cell) => cell.webview.push(webview_index),
                None => matches.push(CellMatches { handle, index, uri: model.cells[index].uri.clone(), content: Vec::new(), webview: vec![webview_index] }),
            }
        }
        matches.sort_by_key(|cell| cell.index);
        self.notebook_find_set(tab, matches, cx);
    }

    /// `_research`'s choice of the current match: the one it was on when its cell still has matches, else the first
    /// match at or after the focused cell (or the old current match's cell).
    fn notebook_find_set(&mut self, tab: u32, matches: Vec<CellMatches>, cx: &mut Context<Self>) {
        let focus = self.host.notebooks.editor(tab).map_or(0, |editor| editor.focus);
        let find = &mut self.notebook_views.find;
        let old = find.current.and_then(|current| find.locate(current)).map(|(cell, within)| (find.matches[cell].handle, within, find.matches[cell].content.get(within).map(|found| found.range.start)));
        find.current = match (matches.is_empty(), old) {
            (true, _) => None,
            (false, None) => {
                let cell = matches.iter().position(|cell| cell.index >= focus).unwrap_or(matches.len()) % matches.len();
                Some(NotebookFind::count_before(&matches, cell))
            }
            (false, Some((handle, within, start))) => {
                let old_index = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)).and_then(|model| model.index_of_handle(handle)).unwrap_or(focus);
                let cell = matches.iter().position(|cell| cell.index >= old_index).unwrap_or(matches.len()) % matches.len();
                let within = match (matches[cell].handle == handle, start) {
                    (true, Some(start)) => matches[cell].content.iter().position(|found| found.range.start >= start).unwrap_or(within.min(matches[cell].content.len())),
                    _ => 0,
                };
                Some(NotebookFind::count_before(&matches, cell) + within)
            }
        };
        find.matches = matches;
        self.notebook_find_update_editing(tab);
        self.notebook_find_highlight_current(tab);
        cx.notify();
    }

    /// findModel.ts `_onFindReplaceStateChange` `updateEditingState`: with the replace input shown, markup cells with
    /// source matches open in edit mode, the others find opened go back to their preview.
    fn notebook_find_update_editing(&mut self, tab: u32) {
        let find = &mut self.notebook_views.find;
        let Some(view) = self.notebook_views.views.get_mut(&tab) else { return };
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return };
        let wanted: HashSet<u64> = match find.state.is_revealed && find.state.is_replace_revealed {
            true => find.matches.iter().filter(|cell| !cell.content.is_empty()).map(|cell| cell.handle).collect(),
            false => HashSet::new(),
        };
        for cell in model.cells.iter().filter(|cell| cell.kind == MARKUP) {
            let mine = find.editing.contains(&cell.handle);
            if view.editing.contains(&cell.handle) && !mine {
                continue;
            }
            match wanted.contains(&cell.handle) {
                true => {
                    if view.editing.insert(cell.handle) {
                        find.editing.insert(cell.handle);
                    }
                }
                false if mine => {
                    view.editing.remove(&cell.handle);
                    find.editing.remove(&cell.handle);
                }
                false => {}
            }
        }
    }

    /// `highlightCurrentFindMatchDecoration`: a source match is drawn in its cell; a webview match is highlighted by
    /// the preloads, which answer with its offset.
    fn notebook_find_highlight_current(&mut self, tab: u32) {
        let find = &self.notebook_views.find;
        let Some((cell, within)) = find.current.and_then(|current| find.locate(current)) else { return };
        let matches = &find.matches[cell];
        if within < matches.content.len() {
            return;
        }
        let index = matches.webview[within - matches.content.len()];
        if let Some(layer) = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()) {
            layer.post(json!({ "type": "findHighlightCurrent", "index": index, "ownerID": OWNER_ID }));
        }
    }

    /// `find({ previous })`: the next or previous match, wrapping, focused and revealed (`revealCellRange`).
    pub(in crate::workbench) fn notebook_find_move(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tab, _)) = self.active_notebook_tab() else { return };
        let find = &mut self.notebook_views.find;
        let total = find.total();
        if total == 0 {
            return;
        }
        if let Some((cell, within)) = find.current.and_then(|current| find.locate(current)) {
            let matches = &find.matches[cell];
            if within >= matches.content.len() {
                let index = matches.webview[within - matches.content.len()];
                if let Some(layer) = self.notebook_views.views.get_mut(&tab).and_then(|view| view.webview.as_mut()) {
                    layer.post(json!({ "type": "findUnHighlightCurrent", "index": index, "ownerID": OWNER_ID }));
                }
            }
        }
        let find = &mut self.notebook_views.find;
        find.current = Some(match find.current {
            None if previous => total - 1,
            None => 0,
            Some(current) if previous => (current + total - 1) % total,
            Some(current) => (current + 1) % total,
        });
        self.notebook_find_highlight_current(tab);
        self.notebook_find_reveal(tab, window, cx);
        cx.notify();
    }

    /// `revealCellRange`: the match's cell takes the focus (not the keyboard); a source match is selected in its editor
    /// and scrolled into the middle of the list when outside it, a markup cell opening in edit mode for it.
    fn notebook_find_reveal(&mut self, tab: u32, window: &mut Window, cx: &mut Context<Self>) {
        let find = &self.notebook_views.find;
        let Some((cell, within)) = find.current.and_then(|current| find.locate(current)) else { return };
        let (handle, index) = (find.matches[cell].handle, find.matches[cell].index);
        let range = find.matches[cell].content.get(within).map(|found| found.range.clone());
        if range.is_some() {
            let kind = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)).map(|model| model.cells[index].kind);
            if kind == Some(MARKUP) && let Some(view) = self.notebook_views.views.get_mut(&tab) && view.editing.insert(handle) {
                self.notebook_views.find.editing.insert(handle);
            }
        }
        let keyboard = window.focused(cx);
        self.focus_notebook_cell(tab, handle, false, window, cx);
        if let Some(keyboard) = keyboard {
            window.focus(&keyboard, cx);
        }
        let Some(range) = range else {
            self.reveal_notebook_cells(tab, index, index + 1, CellReveal::CenterIfOutside, cx);
            return;
        };
        if let Some(editor) = self.editors.active_mut() {
            editor.set_selections(vec![Selection { anchor: range.start, head: range.end }]);
            let editor = self.editors.active().expect("set above");
            self.host.send_selection(editor, "api");
        }
        self.reveal_notebook_line(tab, index, range.start.line, cx);
        cx.notify();
    }

    /// `revealRangeInCenterIfOutsideViewportAsync` on a cell's line.
    fn reveal_notebook_line(&mut self, tab: u32, index: usize, line: usize, cx: &mut Context<Self>) {
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return };
        let rows = self.notebook_rows(tab, model);
        let Some(row) = rows.iter().find(|row| row.index == index) else { return };
        let line_height = self.notebook_line_height();
        let top = row.layout.top + row.layout.editor_top + layout::EDITOR_TOP_PADDING + line as f32 * line_height;
        let view = self.notebook_views.views.entry(tab).or_default();
        let height = f32::from(view.bounds.get().size.height);
        if top < view.scroll_top || top + line_height > view.scroll_top + height {
            self.set_list_scroll(ListId::Notebook(tab), point(0., top + line_height / 2. - height / 2.), cx);
        }
    }

    /// `didFindHighlightCurrent`: the highlighted output or preview match, `offset` into its cell, in the middle of the
    /// list (`revealCellOffsetInCenter`).
    pub(in crate::workbench) fn notebook_find_did_highlight(&mut self, tab: u32, offset: f32, cx: &mut Context<Self>) {
        let find = &self.notebook_views.find;
        let Some((cell, _)) = find.current.and_then(|current| find.locate(current)) else { return };
        let index = find.matches[cell].index;
        let Some(model) = self.host.notebooks.editor(tab).and_then(|editor| self.host.notebooks.model(&editor.notebook)) else { return };
        let rows = self.notebook_rows(tab, model);
        let Some(row) = rows.iter().find(|row| row.index == index) else { return };
        let top = row.layout.top + offset;
        let height = f32::from(self.notebook_views.views.entry(tab).or_default().bounds.get().size.height);
        self.set_list_scroll(ListId::Notebook(tab), point(0., top - height / 2.), cx);
    }

    /// findMatchDecorationModel.ts's overview ruler decorations in the center lane: every cell with matches in
    /// `findMatchForeground`, then the current match in `selectionHighlightForeground`.
    pub(in crate::workbench) fn notebook_find_ruler_decorations(&self, tab: u32) -> Vec<RulerDecoration> {
        let find = &self.notebook_views.find;
        if find.tab != Some(tab) {
            return Vec::new();
        }
        let mut decorations = Vec::new();
        for cell in &find.matches {
            let lines = cell.content.iter().map(|found| found.range.start.line).collect();
            decorations.push(RulerDecoration { cell: cell.index, color: "editorOverviewRuler.findMatchForeground", lines, include_output: !cell.webview.is_empty() });
        }
        if let Some((cell, within)) = find.current.and_then(|current| find.locate(current)) {
            let matches = &find.matches[cell];
            let lines = matches.content.get(within).map(|found| vec![found.range.start.line]);
            let include_output = lines.is_none();
            decorations.push(RulerDecoration { cell: matches.index, color: "editorOverviewRuler.selectionHighlightForeground", lines: lines.unwrap_or_default(), include_output });
        }
        decorations
    }

    fn notebook_find_replace_pattern(&self) -> ReplacePattern {
        let state = &self.notebook_views.find.state;
        match state.is_regex {
            true => parse_replace_string(&state.replace_string),
            false => ReplacePattern::from_static(&state.replace_string),
        }
    }

    /// `replaceOne`: the current source match takes the replace string; an output or preview match cannot.
    pub(in crate::workbench) fn notebook_find_replace_one(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notebook_views.find.total() == 0 {
            return;
        }
        if self.notebook_views.find.current.is_none() {
            self.notebook_find_move(false, window, cx);
        }
        let find = &self.notebook_views.find;
        let Some((cell, within)) = find.current.and_then(|current| find.locate(current)) else { return };
        let Some(found) = find.matches[cell].content.get(within) else { return };
        let text = self.notebook_find_replace_pattern().build(found.matches.as_deref(), find.state.preserve_case);
        let edits = vec![(found.range.clone(), text)];
        let uri = find.matches[cell].uri.clone();
        self.notebook_find_edit_cell(&uri, edits);
        self.notebook_find_research(cx);
        cx.notify();
    }

    /// `replaceAll`: every source match of every cell.
    pub(in crate::workbench) fn notebook_find_replace_all(&mut self, cx: &mut Context<Self>) {
        let pattern = self.notebook_find_replace_pattern();
        let find = &self.notebook_views.find;
        let preserve_case = find.state.preserve_case;
        let cells: Vec<(Value, Vec<(Range<Position>, String)>)> = find
            .matches
            .iter()
            .filter(|cell| !cell.content.is_empty())
            .map(|cell| (cell.uri.clone(), cell.content.iter().map(|found| (found.range.clone(), pattern.build(found.matches.as_deref(), preserve_case))).collect()))
            .collect();
        for (uri, edits) in cells {
            self.notebook_find_edit_cell(&uri, edits);
        }
        self.notebook_find_research(cx);
        cx.notify();
    }

    /// One edit of a cell's text model, sent to the extension host as any model change.
    fn notebook_find_edit_cell(&mut self, uri: &Value, edits: Vec<(Range<Position>, String)>) {
        let Ok((scheme, path)) = super::uri_parts(uri) else { return };
        let Some(editor) = self.editors.model_mut(scheme, &path) else { return };
        let edits = edits.into_iter().map(|(range, text)| (range.start, range.end, text, false)).collect();
        let Some(change) = editor.execute_edits(edits, None, crate::editor::EditReason::Unknown("bulkEdit")) else { return };
        let editor = self.editors.model(scheme, &path).expect("edited above");
        self.host.send_model_change(&self.editors, editor, &change);
    }

    /// The find matches of a cell editor: the source matches and which of them is the current one
    /// (notebookFindDecorations.ts `findMatch` and `currentFindMatch`).
    pub(in crate::workbench) fn notebook_find_cell_matches(&self, editor: &crate::editor::OpenEditor) -> Option<(Vec<Range<Position>>, Option<usize>)> {
        if !self.notebook_find_visible() || editor.document().scheme != crate::notebook::CELL_SCHEME {
            return None;
        }
        let find = &self.notebook_views.find;
        let current = find.current.and_then(|current| find.locate(current));
        let (position, cell) = find.matches.iter().enumerate().find(|(_, cell)| super::uri_parts(&cell.uri).is_ok_and(|(scheme, path)| editor.document().is(scheme, &path)))?;
        let highlighted = current.filter(|(cell, _)| *cell == position).map(|(_, within)| within).filter(|within| *within < cell.content.len());
        Some((cell.content.iter().map(|found| found.range.clone()).collect(), highlighted))
    }

    /// notebookFindReplaceWidget.css `.simple-fr-find-part-wrapper`: the widget at the list's top right, 18px from
    /// its right edge, under the notebook toolbar (`notebook.globalToolbar` puts it 26px down the editor).
    pub(in crate::workbench) fn notebook_find_widget(&self, tab: u32, list_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let find = &self.notebook_views.find;
        if !find.state.is_revealed || find.tab != Some(tab) {
            return None;
        }
        let in_interactive = self.active_interactive().is_some();
        let replace = find.state.is_replace_revealed && !in_interactive;
        let foreground = self.foreground_or("editorWidget.foreground");
        let disabled = self.color("disabledForeground");
        let total = find.total();
        let label = self.notebook_find_count_label();
        let label_color = if !find.state.search_string.is_empty() && total == 0 { self.color("errorForeground") } else { foreground };
        let width = WIDGET_WIDTH.min(list_width - 28. - 28. - 8. - 2. * WIDGET_PADDING);
        let filtered = !(find.filters.markup_source && find.filters.markup_preview && find.filters.code_source && find.filters.code_output);
        let current_is_source = find.current.and_then(|current| find.locate(current)).is_some_and(|(cell, within)| within < find.matches[cell].content.len());
        let any_webview = find.matches.iter().any(|cell| !cell.webview.is_empty());

        let option = |icon: &'static str, checked: bool, enabled: bool, toggle: fn(&mut FindState), cx: &mut Context<Self>| {
            let color = if checked { self.foreground_or("inputOption.activeForeground") } else { foreground };
            self.icon(icon, 20., 16., color)
                .id(icon)
                .ml(px(2.))
                .rounded(px(3.))
                .border_1()
                .border_color(if checked { self.color("inputOption.activeBorder") } else { gpui::transparent_black().into() })
                .when(checked, |toggle| toggle.bg(self.color("inputOption.activeBackground")))
                .when(!enabled, |toggle| toggle.opacity(0.3))
                .when(enabled, |toggle| toggle.cursor_pointer().when(!checked, |toggle| toggle.hover(|toggle| toggle.bg(self.color("inputOption.hoverBackground")))))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .when(enabled, |element| {
                    element.on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                        toggle(&mut workbench.notebook_views.find.state);
                        workbench.notebook_find_research(cx);
                    }))
                })
        };
        let button = |icon: &'static str, enabled: bool, action: fn(&mut Workbench, &mut Window, &mut Context<Workbench>), cx: &mut Context<Self>| {
            self.icon(icon, 20., 16., if enabled { foreground } else { disabled })
                .id(icon)
                .ml(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(gpui::transparent_black())
                .when(!enabled, |button| button.opacity(0.3))
                .when(enabled, |button| {
                    button
                        .cursor_pointer()
                        .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| action(workbench, window, cx)))
                })
        };
        let input = |id: InputId, placeholder: &str, focus: &FocusHandle, controls: Div, cx: &mut Context<Self>| {
            let focused = focus.is_focused(window);
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(26.))
                .flex()
                .items_center()
                .bg(self.color("input.background"))
                .border_1()
                .border_color(if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(gpui::transparent_black().into()) })
                .rounded(px(4.))
                .text_color(self.foreground_or("input.foreground"))
                .cursor_text()
                .child(div().flex_1().min_w_0().h_full().ml(px(6.)).flex().child(self.input_text(id, InputLook { password: false, placeholder: Some(placeholder.into()), focused }, cx)))
                .child(controls.mr(px(1.)))
        };
        let filter_button = self
            .icon("filter", 20., 16., if filtered { self.foreground_or("inputOption.activeForeground") } else { foreground })
            .id("notebook-find-filters")
            .ml(px(2.))
            .rounded(px(3.))
            .border_1()
            .border_color(if filtered { self.color("inputOption.activeBorder") } else { gpui::transparent_black().into() })
            .when(filtered, |button| button.bg(self.color("inputOption.activeBackground")))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_notebook_find_filters(event.position, window, cx);
            }))
            .child(self.hover_target("notebook-find-filters-hover", super::HoverContent::Text("Find Filters".into()), super::HoverOptions::mouse()));
        let state = &find.state;
        let find_controls = div()
            .flex()
            .items_center()
            .child(option("case-sensitive", state.match_case, true, |state| state.match_case = !state.match_case, cx))
            .child(option("whole-word", state.whole_word, true, |state| state.whole_word = !state.whole_word, cx))
            .child(option("regex", state.is_regex, !filtered, |state| state.is_regex = !state.is_regex, cx))
            .child(filter_button);
        let find_part = div()
            .ml(px(17.))
            .p(px(4.))
            .flex()
            .items_center()
            .child(input(InputId::NotebookFind, "Find", &find.find_focus, find_controls, cx))
            .child(div().min_w(px(69.)).flex().justify_center().whitespace_nowrap().text_color(label_color).child(label))
            .child(button("arrow-up", total > 0, |workbench, window, cx| workbench.notebook_find_move(true, window, cx), cx))
            .child(button("arrow-down", total > 0, |workbench, window, cx| workbench.notebook_find_move(false, window, cx), cx))
            .child(
                self.icon("list-selection", 20., 16., foreground)
                    .id("notebook-find-selection")
                    .ml(px(3.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(gpui::transparent_black()),
            )
            .child(button("close", true, |workbench, window, cx| workbench.hide_notebook_find(window, cx), cx));
        let replace_part = replace.then(|| {
            let preserve = div().flex().items_center().child(option("preserve-case", state.preserve_case, true, |state| state.preserve_case = !state.preserve_case, cx));
            div()
                .ml(px(17.))
                .p(px(4.))
                .flex()
                .items_center()
                .child(input(InputId::NotebookReplace, "Replace", &find.replace_focus, preserve, cx))
                .child(button("replace", current_is_source, |workbench, window, cx| workbench.notebook_find_replace_one(window, cx), cx))
                .child(button("replace-all", total > 0 && !any_webview, |workbench, _, cx| workbench.notebook_find_replace_all(cx), cx))
        });
        let toggle = self
            .icon(if replace { "chevron-down" } else { "chevron-right" }, 18., 16., if in_interactive { disabled } else { foreground })
            .id("notebook-find-toggle-replace")
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(WIDGET_PADDING))
            .w(px(18.))
            .when(!in_interactive, |toggle| {
                toggle
                    .cursor_pointer()
                    .hover(|toggle| toggle.bg(self.color("toolbar.hoverBackground")))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| {
                        let state = &mut workbench.notebook_views.find.state;
                        state.is_replace_revealed = !state.is_replace_revealed;
                        if let Some((tab, _)) = workbench.active_notebook_tab() {
                            workbench.notebook_find_update_editing(tab);
                        }
                        cx.notify();
                    }))
            });
        Some(
            div()
                .id("notebook-find-widget")
                .debug_selector(|| "notebook-find-widget".into())
                .absolute()
                .top(px(26. - super::TOOLBAR_HEIGHT))
                .right(px(18.))
                .w(px(width + 2. * WIDGET_PADDING))
                .px(px(WIDGET_PADDING))
                .occlude()
                .cursor_default()
                .font_family(".SystemUIFont")
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(foreground)
                .bg(self.color("editorWidget.background"))
                .rounded_bl(px(4.))
                .rounded_br(px(4.))
                .shadow(vec![BoxShadow { color: gpui::rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(6.), spread_radius: px(0.), inset: false }])
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(toggle)
                .child(find_part)
                .children(replace_part),
        )
    }

    /// `_updateMatchesCount`: "{current} of {count}", "?" without a current match.
    pub(in crate::workbench) fn notebook_find_count_label(&self) -> String {
        let find = &self.notebook_views.find;
        match find.total() {
            0 => "No results".to_owned(),
            count => {
                let position = find.current.map_or("?".to_owned(), |current| (current + 1).to_string());
                let plus = if count >= MATCHES_LIMIT { "+" } else { "" };
                format!("{position} of {count}{plus}")
            }
        }
    }

    /// The widget's bounds in the list, for the back layer to leave uncovered.
    pub(in crate::workbench) fn notebook_find_widget_bounds(&self, tab: u32, list_width: f32) -> Option<gpui::Bounds<gpui::Pixels>> {
        let find = &self.notebook_views.find;
        if !find.state.is_revealed || find.tab != Some(tab) {
            return None;
        }
        let width = WIDGET_WIDTH.min(list_width - 28. - 28. - 8. - 2. * WIDGET_PADDING) + 2. * WIDGET_PADDING;
        let rows = if find.state.is_replace_revealed && self.active_interactive().is_none() { 2. } else { 1. };
        // 12px for the shadow.
        Some(gpui::Bounds::new(point(px(list_width - 18. - width - 12.), px(0.)), gpui::size(px(width + 24.), px(rows * 34. + 12.))))
    }

    /// `NotebookFindFilterActionViewItem`: the filters as checked menu items.
    fn show_notebook_find_filters(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let filters = self.notebook_views.find.filters;
        let actions = [("markupSource", "Markdown Source", filters.markup_source), ("markupPreview", "Rendered Markdown", filters.markup_preview), ("codeSource", "Code Cell Source", filters.code_source), ("codeOutput", "Code Cell Output", filters.code_output)]
            .into_iter()
            .map(|(key, title, checked)| crate::menus::MenuAction::Command { id: format!("_notebook.find.filter.{key}"), title: title.into(), enabled: true, checked, alt: None })
            .collect();
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, gpui::Bounds::new(position, gpui::size(px(0.), px(0.))), Vec::new(), window, cx);
    }
}


