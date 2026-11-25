//! The multi diff editor's input and view model: multiDiffEditorInput.ts (name, resources), multiDiffEditorViewModel.ts
//! (each file's collapsed state) and the rows each file's read-only diff draws, with its unchanged regions hidden as
//! multiDiffEditor's `hideUnchangedRegions: { enabled: true }` does. Drawn by
//! `workbench/editor_view/multi_diff_editor_view.rs`.

use std::cell::Cell;
use std::rc::Rc;

use diff::{DiffOptions, LineRange};
use serde_json::{Value, json};

use crate::diff_editor::unchanged_regions::UnchangedRegion;
use crate::diff_editor::{ComputedDiff, Side, range_alignment};
use crate::editor::{OpenEditor, split_query, uri_string, uri_value};

/// The scheme of a multi diff editor's tab entry; its `MultiDiffInput` names the models it shows.
pub const MULTI_DIFF_EDITOR_SCHEME: &str = "multi-diff-editor";

/// multiDiffEditorOptions.ts standard variant `headerHeight`.
pub const HEADER_HEIGHT: f32 = 40.;

/// The lines of a side without a resource: an empty model.
pub static EMPTY_MODEL: [String; 1] = [String::new()];

/// `MultiDiffEditorInput`: the source the editor is matched by, the preferred label and the files.
pub struct MultiDiffInput {
    pub source: Option<Side>,
    pub label: Option<String>,
    pub files: Vec<FileDiff>,
    pub scroll_top: f32,
    /// The list's height as last laid out.
    pub view_height: Rc<Cell<f32>>,
    /// The source's context keys for every file's toolbar (`IResolvedMultiDiffSource.contextKeys`).
    pub context_keys: Vec<(String, Value)>,
    /// The active file and the 1-based modified line its editor's cursor is on (`activeDiffItem`).
    pub cursor: Option<(usize, usize)>,
}

/// `DocumentDiffItemViewModel`: one file's sides (a missing one is an added or deleted file), where Open File goes,
/// whether it is collapsed, its diff and unchanged regions.
pub struct FileDiff {
    pub original: Option<Side>,
    pub modified: Option<Side>,
    pub go_to_file: Option<Value>,
    pub collapsed: bool,
    pub diff: Option<ComputedDiff>,
    pub unchanged: Vec<UnchangedRegion>,
    unchanged_key: Option<((u32, u32), usize, usize)>,
    /// A side's model did not load (`isBinary`): the file shows the binary file placeholder.
    pub unloadable: bool,
}

/// One row of a file's diff: lines of the original and modified models (0-based) drawn side by side, `None` where
/// the side has a fill (side by side) or nothing (inline: deleted lines have no modified line, inserted ones no
/// original), or the placeholder of a hidden unchanged region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Line { original: Option<usize>, modified: Option<usize> },
    Hidden { region: usize, lines: usize },
}

impl MultiDiffInput {
    pub fn new(source: Option<Side>, label: Option<String>, files: Vec<FileDiff>) -> Self {
        MultiDiffInput { source, label, files, scroll_top: 0., view_height: Rc::default(), context_keys: Vec::new(), cursor: None }
    }

    /// `MultiDiffEditorInput` name: the label, else "Multi Diff Editor", with the file count.
    pub fn name(&self) -> String {
        let label = self.label.as_deref().unwrap_or("Multi Diff Editor");
        match self.files.len() {
            1 => format!("{label} (1 file)"),
            count => format!("{label} ({count} files)"),
        }
    }

    /// Whether a file shows `model`, which then stays open.
    pub fn shows(&self, model: &OpenEditor) -> bool {
        self.files.iter().flat_map(|file| [&file.original, &file.modified]).flatten().any(|side| model.document().is(&side.scheme, &side.path))
    }

    /// `multiDiffEditorAllCollapsed`.
    pub fn all_collapsed(&self) -> bool {
        self.files.iter().all(|file| file.collapsed)
    }

    /// `collapseAll` and `expandAll`.
    pub fn set_all_collapsed(&mut self, collapsed: bool) {
        for file in &mut self.files {
            file.collapsed = collapsed;
        }
    }

    /// Replaces the files when another open asks for other resources; the same ones keep their state.
    pub fn set_files(&mut self, files: Vec<FileDiff>) {
        let same = |a: &FileDiff, b: &FileDiff| a.original == b.original && a.modified == b.modified;
        if self.files.len() != files.len() || self.files.iter().zip(&files).any(|(a, b)| !same(a, b)) {
            self.files = files;
            self.scroll_top = 0.;
            self.cursor = None;
        }
    }

    /// mainThreadEditorTabs.ts `_buildTabObject` of a `MultiDiffEditorInput`: its files with both sides as
    /// `TextDiffInput`s, its resource the multi diff source.
    pub fn tab_dto(&self, group: u32, entry: &OpenEditor, active: bool) -> Value {
        let resource = match &self.source {
            Some(source) => uri_string(&source.scheme, &source.path),
            None => entry.document().uri_string(),
        };
        let uri = |side: &Side| uri_value(&side.scheme, &side.path);
        let diff_editors: Vec<Value> = self
            .files
            .iter()
            .filter_map(|file| Some(json!({ "kind": 2, "original": uri(file.original.as_ref()?), "modified": uri(file.modified.as_ref()?) })))
            .collect();
        json!({
            "id": format!("{group}~default-workbench.input.multiDiffEditor-{resource} "),
            "label": self.name(),
            "input": { "kind": 11, "diffEditors": diff_editors },
            "editorId": "default",
            "isActive": active,
            "isPinned": false,
            "isPreview": entry.preview,
            "isDirty": false,
        })
    }
}

impl MultiDiffInput {
    /// multiDiffEditorWidgetImpl.ts `_navigateToChange`: the next (or previous) change of the active file after (before)
    /// the cursor, else the first (last) change of the next (previous) file, wrapping; without an active file the first
    /// file. The target file expands and its line is revealed; returns the file and its change's line, none without one.
    pub fn navigate(&mut self, next: bool) -> Option<(usize, Option<usize>)> {
        let count = self.files.len();
        if count == 0 {
            return None;
        }
        let target = match self.cursor {
            None => (0, self.files[0].edge_change(true)),
            Some((index, line)) => {
                let changes = self.files[index].modified_changes();
                let within = match next {
                    true => changes.iter().find(|range| range.start > line),
                    false => changes.iter().rev().find(|range| range.end <= line && range.start < line),
                };
                match within {
                    Some(range) => (index, Some(range.start)),
                    None => {
                        let other = if next { (index + 1) % count } else { (index + count - 1) % count };
                        (other, self.files[other].edge_change(next))
                    }
                }
            }
        };
        self.reveal(target.0, target.1);
        Some(target)
    }

    /// Makes `file` the active one with its cursor on `line` (the first line without one), expanded and the line
    /// out of any hidden region.
    pub fn reveal(&mut self, file: usize, line: Option<usize>) {
        let line = line.unwrap_or(1);
        let target = &mut self.files[file];
        target.collapsed = false;
        if let Some(region) = target.unchanged.iter_mut().find(|region| region.hidden_modified().contains(line)) {
            region.show_modified_line(line, crate::diff_editor::unchanged_regions::RevealPreference::FromCloserSide);
        }
        self.cursor = Some((file, line));
    }
}

/// multiDiffEditorWidgetImpl.ts sticky header: the header's top in the list, at the viewport's top while the file
/// scrolls under it, pushed up by the file's end.
pub fn sticky_header_top(file_top: f32, file_height: f32, scroll_top: f32) -> f32 {
    file_top + (scroll_top - file_top).clamp(0., (file_height - HEADER_HEIGHT).max(0.))
}

impl FileDiff {
    pub fn new(original: Option<Side>, modified: Option<Side>, go_to_file: Option<Value>) -> Self {
        FileDiff { original, modified, go_to_file, collapsed: false, diff: None, unchanged: Vec::new(), unchanged_key: None, unloadable: false }
    }

    /// The modified ranges of the changes, 1-based.
    fn modified_changes(&self) -> Vec<LineRange> {
        self.diff.as_ref().map_or(Vec::new(), |diff| diff.diff.changes.iter().map(|change| change.modified).collect())
    }

    /// The first (or last) change's modified line.
    fn edge_change(&self, first: bool) -> Option<usize> {
        let changes = self.modified_changes();
        let change = if first { changes.first() } else { changes.last() };
        change.map(|range| range.start)
    }

    /// Whether 1-based `line` of a side is in an unchanged region (`diff-unchanged-lines`).
    pub fn in_unchanged(&self, side: crate::diff_editor::ZoneSide, line: usize) -> bool {
        self.unchanged.iter().any(|region| match side {
            crate::diff_editor::ZoneSide::Original => region.original_range().contains(line),
            crate::diff_editor::ZoneSide::Modified => region.modified_range().contains(line),
        })
    }

    /// The modified file's name and folder, else the original's (diffEditorItemTemplate.ts `setUri`).
    pub fn label_side(&self) -> Option<&Side> {
        self.modified.as_ref().or(self.original.as_ref())
    }

    /// diffEditorItemTemplate.ts status flag: `R` renamed, `D` deleted, `A` added.
    pub fn status(&self) -> Option<&'static str> {
        match (&self.original, &self.modified) {
            (Some(original), Some(modified)) if split_query(&original.scheme, &original.path).0 != split_query(&modified.scheme, &modified.path).0 => Some("R"),
            (Some(_), None) => Some("D"),
            (None, Some(_)) => Some("A"),
            _ => None,
        }
    }

    /// Recomputes the diff when either side's version or an option changed, then the unchanged regions
    /// (`min_hidden` and `min_context` lines) of a new diff. Each side is its lines and version.
    pub fn update(&mut self, original: (&[String], u32), modified: (&[String], u32), options: DiffOptions, min_hidden: usize, min_context: usize) {
        let versions = (original.1, modified.1);
        let key = (options.ignore_trim_whitespace, options.max_computation_time_ms, options.compute_moves);
        if !self.diff.as_ref().is_some_and(|diff| diff.versions == versions && diff.options == key) {
            let diff = diff::compute_diff(original.0, modified.0, options);
            self.diff = Some(ComputedDiff { versions, options: key, diff });
        }
        let unchanged_key = (versions, min_hidden, min_context);
        if self.unchanged_key == Some(unchanged_key) {
            return;
        }
        let changes = &self.diff.as_ref().expect("computed above").diff.changes;
        let (original_count, modified_count) = (original.0.len(), modified.0.len());
        self.unchanged = UnchangedRegion::from_diffs(changes, original_count, modified_count, min_hidden, min_context);
        // hideUnchangedRegionsFeature.ts: one region covering the whole modified model (no change) shows nothing.
        if let [only] = self.unchanged[..]
            && only.modified_line == 1
            && only.line_count == modified_count
        {
            self.unchanged.clear();
        }
        self.unchanged_key = Some(unchanged_key);
    }

    /// The rows of the diff: side by side each changed stretch of `range_alignment` with the shorter side filled,
    /// inline each change's deleted lines above its inserted ones; the unchanged lines between, hidden regions as
    /// one placeholder row each.
    pub fn rows(&self, original_lines: &[String], modified_count: usize, side_by_side: bool) -> Vec<Row> {
        let Some(computed) = &self.diff else { return Vec::new() };
        let mut rows = Vec::new();
        let (mut original, mut modified) = (1, 1);
        for stretch in range_alignment(&computed.diff, original_lines, side_by_side) {
            self.push_unchanged(&mut rows, original, modified, stretch.modified.start - modified);
            let (original_len, modified_len) = (stretch.original.len(), stretch.modified.len());
            let line = |range: LineRange, offset: usize| (offset < range.len()).then_some(range.start - 1 + offset);
            if side_by_side {
                for offset in 0..original_len.max(modified_len) {
                    rows.push(Row::Line { original: line(stretch.original, offset), modified: line(stretch.modified, offset) });
                }
            } else {
                rows.extend((0..original_len).map(|offset| Row::Line { original: line(stretch.original, offset), modified: None }));
                rows.extend((0..modified_len).map(|offset| Row::Line { original: None, modified: line(stretch.modified, offset) }));
            }
            (original, modified) = (stretch.original.end, stretch.modified.end);
        }
        self.push_unchanged(&mut rows, original, modified, (modified_count + 1).saturating_sub(modified));
        rows
    }

    /// `count` unchanged lines from 1-based `original` and `modified`, a hidden region's lines as its placeholder.
    fn push_unchanged(&self, rows: &mut Vec<Row>, original: usize, modified: usize, count: usize) {
        let mut offset = 0;
        while offset < count {
            let line = modified + offset;
            let hidden = self.unchanged.iter().enumerate().filter(|(_, region)| !region.is_fully_revealed()).find(|(_, region)| region.hidden_modified().start == line);
            if let Some((region, hidden)) = hidden {
                let lines = hidden.hidden_modified().len();
                rows.push(Row::Hidden { region, lines });
                offset += lines;
                continue;
            }
            rows.push(Row::Line { original: Some(original - 1 + offset), modified: Some(line - 1) });
            offset += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    fn side(path: &str) -> Option<Side> {
        Some(Side { scheme: "file".into(), path: PathBuf::from(path) })
    }

    fn computed(original: &[String], modified: &[String]) -> FileDiff {
        let mut file = FileDiff::new(side("/a.txt"), side("/a.txt"), None);
        file.update((original, 1), (modified, 1), DiffOptions::default(), 3, 3);
        file
    }

    fn line(original: Option<usize>, modified: Option<usize>) -> Row {
        Row::Line { original, modified }
    }

    #[test]
    fn side_by_side_fills_the_shorter_side_and_inline_stacks_deleted_over_inserted() {
        let (original, modified) = (lines("a\nb\nc"), lines("a\nB\nx\nc"));
        let file = computed(&original, &modified);
        assert_eq!(file.rows(&original, modified.len(), true), vec![line(Some(0), Some(0)), line(Some(1), Some(1)), line(None, Some(2)), line(Some(2), Some(3))]);
        assert_eq!(file.rows(&original, modified.len(), false), vec![line(Some(0), Some(0)), line(Some(1), None), line(None, Some(1)), line(None, Some(2)), line(Some(2), Some(3))]);
    }

    #[test]
    fn unchanged_regions_hide_behind_one_row_until_revealed() {
        let original: Vec<String> = (1..=20).map(|line| format!("line {line}")).collect();
        let mut modified = original.clone();
        modified[9] = "changed".into();
        let mut file = computed(&original, &modified);
        let rows = file.rows(&original, modified.len(), true);
        // Lines 1-6 and 14-20 hide, 3 context lines on each side of line 10.
        assert_eq!(rows[0], Row::Hidden { region: 0, lines: 6 });
        assert_eq!(rows[1], line(Some(6), Some(6)));
        assert_eq!(rows[8], Row::Hidden { region: 1, lines: 7 });
        assert_eq!(rows.len(), 9);
        file.unchanged[0].show_all();
        assert_eq!(file.rows(&original, modified.len(), true).len(), 14);
    }

    #[test]
    fn an_added_file_diffs_against_an_empty_model() {
        let modified = lines("one\ntwo");
        let mut file = FileDiff::new(None, side("/new.txt"), None);
        file.update((&EMPTY_MODEL, 0), (&modified, 1), DiffOptions::default(), 3, 3);
        assert_eq!(file.status(), Some("A"));
        assert_eq!(file.rows(&EMPTY_MODEL, modified.len(), true), vec![line(Some(0), Some(0)), line(None, Some(1))]);
    }

    #[test]
    fn name_counts_files_and_collapse_all_flips_every_file() {
        let mut input = MultiDiffInput::new(None, Some("Changes".into()), vec![FileDiff::new(side("/a"), side("/a"), None)]);
        assert_eq!(input.name(), "Changes (1 file)");
        input.files.push(FileDiff::new(side("/b"), None, None));
        assert_eq!(input.name(), "Changes (2 files)");
        assert_eq!(input.files[1].status(), Some("D"));
        input.set_all_collapsed(true);
        assert!(input.all_collapsed());
        input.files[0].collapsed = false;
        assert!(!input.all_collapsed());
    }

    #[test]
    fn navigation_walks_the_changes_then_the_next_file_and_wraps() {
        let original: Vec<String> = (1..=20).map(|line| format!("line {line}")).collect();
        let mut modified = original.clone();
        modified[1] = "two".into();
        modified[17] = "eighteen".into();
        let mut input = MultiDiffInput::new(None, None, vec![computed(&original, &modified), computed(&original, &modified)]);
        input.files[1].collapsed = true;
        assert_eq!(input.navigate(true), Some((0, Some(2))));
        // Line 18 sits in no hidden region: 3 context lines after line 2 and before 18 hide lines 6-14.
        assert_eq!(input.navigate(true), Some((0, Some(18))));
        assert_eq!(input.navigate(true), Some((1, Some(2))));
        assert!(!input.files[1].collapsed);
        assert_eq!(input.navigate(false), Some((0, Some(18))));
        assert_eq!(input.navigate(false), Some((0, Some(2))));
        assert_eq!(input.navigate(false), Some((1, Some(18))));
        input.reveal(0, Some(10));
        assert!(input.files[0].unchanged.iter().all(|region| !region.hidden_modified().contains(10)));
    }

    #[test]
    fn the_header_sticks_to_the_viewport_top_within_its_file() {
        assert_eq!(sticky_header_top(100., 300., 0.), 100.);
        assert_eq!(sticky_header_top(100., 300., 150.), 150.);
        assert_eq!(sticky_header_top(100., 300., 1000.), 360.);
    }
}
