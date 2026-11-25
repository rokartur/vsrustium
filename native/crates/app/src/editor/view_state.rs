//! The text file editor's view state (editorWithViewState.ts, editorPane.ts `EditorMemento`): an editor that
//! leaves its group keeps its cursors, scroll and folds per file, and the next editor of that file in the group
//! takes them back. Stored in workspace storage under the text file editor's memento.

use serde_json::{Value, json};

use super::edit::{char_column, utf16_column};
use super::folding::{FoldSource, LineMemento};
use super::{OpenEditor, Position, Selection, Viewport};

/// `Memento.COMMON_PREFIX` with `TEXT_FILE_EDITOR_ID`.
pub const STORAGE_KEY: &str = "memento/workbench.editors.files.textFileEditor";
/// `AbstractTextEditor.VIEW_STATE_PREFERENCE_KEY`.
const MEMENTO_KEY: &str = "textEditorViewState";
/// The limit `AbstractEditorWithViewState` gives its `EditorMemento`.
const LIMIT: usize = 100;
/// The port's one editor group.
const GROUP_ID: &str = "0";
const FOLDING_CONTRIBUTION: &str = "editor.contrib.folding";

/// The `LRUCache<resource, MapGroupToMemento>` of an `EditorMemento`, oldest first.
#[derive(Default)]
pub struct ViewStates {
    entries: Vec<(String, Value)>,
}

impl ViewStates {
    /// `doLoad`: the memento's cache as `LRUCache.fromJSON` reads it.
    pub fn from_memento(raw: &str) -> Self {
        let Ok(memento) = serde_json::from_str::<Value>(raw) else { return Self::default() };
        let mut states = Self::default();
        for entry in memento[MEMENTO_KEY].as_array().into_iter().flatten() {
            if let (Some(resource), Some(groups)) = (entry[0].as_str(), entry.get(1)) {
                states.entries.push((resource.to_owned(), groups.clone()));
            }
        }
        states.entries.truncate(LIMIT);
        states
    }

    /// `saveState`: the cache as `LRUCache.toJSON` writes it into the memento.
    pub fn to_memento(&self) -> String {
        let entries: Vec<Value> = self.entries.iter().map(|(resource, groups)| json!([resource, groups])).collect();
        json!({ MEMENTO_KEY: entries }).to_string()
    }

    /// `saveEditorState`: the newest entry, the oldest beyond the limit dropped.
    pub fn save(&mut self, resource: &str, state: Value) {
        let mut groups = self.take(resource).unwrap_or_else(|| json!({}));
        groups[GROUP_ID] = state;
        self.entries.push((resource.to_owned(), groups));
        if self.entries.len() > LIMIT {
            self.entries.remove(0);
        }
    }

    /// `loadEditorState`, which makes the entry the newest (`Touch.AsNew`).
    pub fn load(&mut self, resource: &str) -> Option<Value> {
        let groups = self.take(resource)?;
        let state = groups.get(GROUP_ID).cloned();
        self.entries.push((resource.to_owned(), groups));
        state
    }

    fn take(&mut self, resource: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(entry, _)| entry == resource)?;
        Some(self.entries.remove(index).1)
    }
}

impl OpenEditor {
    /// `ICodeEditorViewState` (`saveViewState`): `saveCursorState`, `ViewModel.saveState` and the folding
    /// contribution's `saveViewState`.
    pub fn view_state(&self, viewport: Viewport) -> Value {
        let lines = &self.document().lines;
        let position = |position: Position| json!({ "lineNumber": position.line + 1, "column": utf16_column(&lines[position.line], position.column) + 1 });
        let cursor_state: Vec<Value> = self
            .selections
            .iter()
            .map(|selection| json!({ "inSelectionMode": selection.anchor != selection.head, "selectionStart": position(selection.anchor), "position": position(selection.head) }))
            .collect();
        let first_view_line = self.view_line_at(self.scroll.top, viewport).min(self.view_line_count() - 1);
        let first_position = Position::new(self.model_line(first_view_line), 0);
        let mut folding = json!({ "lineCount": lines.len(), "foldedImports": self.folded_imports });
        if let Some(regions) = self.folding.get_memento(lines) {
            folding["collapsedRegions"] = regions.iter().map(line_memento_json).collect();
        }
        json!({
            "cursorState": cursor_state,
            "viewState": {
                "scrollLeft": self.scroll.left,
                "firstPosition": position(first_position),
                "firstPositionDeltaTop": self.line_top(first_view_line, viewport) - self.scroll.top,
            },
            "contributionsState": { FOLDING_CONTRIBUTION: folding },
        })
    }

    /// `restoreViewState`: the cursors, then the folds, then the scroll from the first visible position.
    pub fn restore_view_state(&mut self, state: &Value, viewport: Viewport) {
        let (Some(cursor_state), Some(view_state)) = (state["cursorState"].as_array(), state.get("viewState")) else { return };
        let selections: Vec<Selection> = cursor_state
            .iter()
            .filter_map(|cursor| {
                let head = self.position_from_json(&cursor["position"])?;
                let anchor = match cursor["inSelectionMode"].as_bool() {
                    Some(true) => self.position_from_json(&cursor["selectionStart"])?,
                    _ => head,
                };
                Some(Selection { anchor, head })
            })
            .collect();
        if !selections.is_empty() {
            self.set_selections(selections);
        }
        let folding = &state["contributionsState"][FOLDING_CONTRIBUTION];
        self.folded_imports = folding["foldedImports"].as_bool().unwrap_or(false);
        if let Some(regions) = folding["collapsedRegions"].as_array().filter(|regions| !regions.is_empty()) {
            let regions: Vec<LineMemento> = regions.iter().filter_map(line_memento_from_json).collect();
            self.folding.apply_memento(&self.model.borrow().document.lines, &regions);
        }
        let Some(first_position) = self.position_from_json(&view_state["firstPosition"]) else { return };
        let delta = view_state["firstPositionDeltaTop"].as_f64().unwrap_or(0.) as f32;
        let top = self.line_top(self.view_line(first_position.line), viewport) - delta;
        let left = view_state["scrollLeft"].as_f64().unwrap_or(0.) as f32;
        self.set_scroll(top, left, viewport);
    }

    /// `TextModel.validatePosition` of a 1-based UTF-16 `IPosition`.
    fn position_from_json(&self, position: &Value) -> Option<Position> {
        let lines = &self.document().lines;
        let line = (position["lineNumber"].as_u64()? as usize).clamp(1, lines.len()) - 1;
        let column = char_column(&lines[line], (position["column"].as_u64()? as usize).max(1) - 1);
        Some(Position::new(line, column))
    }
}

fn line_memento_json(memento: &LineMemento) -> Value {
    let source = memento.source.map(|source| match source {
        FoldSource::Provider => 0,
        FoldSource::UserDefined => 1,
        FoldSource::Recovered => 2,
    });
    json!({ "startLineNumber": memento.start, "endLineNumber": memento.end, "isCollapsed": memento.collapsed, "source": source, "checksum": memento.checksum })
}

fn line_memento_from_json(value: &Value) -> Option<LineMemento> {
    let source = value["source"].as_u64().map(|source| match source {
        1 => FoldSource::UserDefined,
        2 => FoldSource::Recovered,
        _ => FoldSource::Provider,
    });
    Some(LineMemento {
        start: value["startLineNumber"].as_u64()? as usize,
        end: value["endLineNumber"].as_u64()? as usize,
        checksum: value["checksum"].as_u64().map(|checksum| checksum as u32),
        collapsed: value["isCollapsed"].as_bool(),
        source,
    })
}

#[cfg(test)]
mod tests;
