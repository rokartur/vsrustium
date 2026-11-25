//! The diff editor's input and view model: diffEditorInput.ts (labels), diffEditorViewModel.ts (the diff of two
//! models, recomputed when either changes) and diffEditorViewZones.ts `computeRangeAlignment` (the whitespace that
//! keeps both sides aligned). Drawn by `workbench/diff_editor_view.rs`.

use std::path::PathBuf;

use diff::{DiffOptions, LineRange, LinesDiff};

use crate::editor::{OpenEditor, split_query};

/// The scheme of a diff editor's tab entry: the group keeps it as an editor without text, its `DiffInput` names the
/// two models it shows.
pub const DIFF_EDITOR_SCHEME: &str = "diff-editor";

/// One side's model, as `EditorGroup::model` finds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub scheme: String,
    pub path: PathBuf,
}

impl Side {
    pub fn name(&self) -> String {
        split_query(&self.scheme, &self.path).0.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }
}

/// `DiffEditorInput`: the original and modified resources and the preferred label. The preferred description is
/// left out: the port's tabs show no descriptions yet.
pub struct DiffInput {
    pub original: Side,
    pub modified: Side,
    pub label: Option<String>,
    pub diff: Option<ComputedDiff>,
    /// The view mode the editor draws now: inline by setting or below the breakpoint.
    pub inline_now: bool,
    /// The view zones of the current diff and view mode, both sides.
    pub zones: Vec<DiffZone>,
    /// The modified side's zones as placed in its model: view zone id and zone.
    pub placed_zones: Vec<(u32, DiffZone)>,
}

/// The diff of the models' versions it was computed for.
pub struct ComputedDiff {
    pub versions: (u32, u32),
    pub options: (bool, u64, bool),
    pub diff: LinesDiff,
}

impl DiffInput {
    pub fn new(original: Side, modified: Side, label: Option<String>) -> Self {
        DiffInput { original, modified, label, diff: None, inline_now: false, zones: Vec::new(), placed_zones: Vec::new() }
    }

    /// `computeLabels` name: the preferred label, else `original ↔ modified`.
    pub fn name(&self) -> String {
        match &self.label {
            Some(label) => label.clone(),
            None => format!("{} \u{2194} {}", self.original.name(), self.modified.name()),
        }
    }

    /// `computeLabels` title (medium verbosity): the sides' paths joined with ` ↔ `, after the preferred label.
    pub fn title(&self, path_label: impl Fn(&Side) -> String) -> String {
        let (original, modified) = (path_label(&self.original), path_label(&self.modified));
        let title = if original == modified { modified } else { format!("{original} \u{2194} {modified}") };
        match &self.label {
            Some(label) => format!("{label} ({title})"),
            None => title,
        }
    }

    /// Recomputes the diff when either model or an option changed since the last one (diffEditorViewModel.ts
    /// `_diff`, which upstream debounces on a worker).
    pub fn update(&mut self, original: &OpenEditor, modified: &OpenEditor, options: DiffOptions) {
        let versions = (original.version_id, modified.version_id);
        let key = (options.ignore_trim_whitespace, options.max_computation_time_ms, options.compute_moves);
        if self.diff.as_ref().is_some_and(|diff| diff.versions == versions && diff.options == key) {
            return;
        }
        let (original_lines, modified_lines) = (&original.document.lines, &modified.document.lines);
        let diff = diff::compute_diff(original_lines, modified_lines, options);
        self.diff = Some(ComputedDiff { versions, options: key, diff });
    }
}

/// `diffEditor.*` settings the view reads, with diffEditor.ts defaults.
pub fn diff_options(setting: impl Fn(&str) -> serde_json::Value) -> DiffOptions {
    DiffOptions {
        ignore_trim_whitespace: setting("diffEditor.ignoreTrimWhitespace").as_bool().unwrap_or(true),
        max_computation_time_ms: setting("diffEditor.maxComputationTime").as_u64().unwrap_or(5000),
        compute_moves: setting("diffEditor.experimental.showMoves").as_bool().unwrap_or(false),
        extend_to_subwords: false,
    }
}

/// `ILineRangeAlignment`: lines of both sides drawn at the same height; `changed` when they belong to a change.
#[derive(Clone, Debug, PartialEq)]
pub struct Alignment {
    pub original: LineRange,
    pub modified: LineRange,
    pub changed: bool,
}

/// `computeRangeAlignment` without other view zones or wrapped lines: per change, the stretches the inner changes
/// break it into when `inner_hunk_alignment` (side by side), else the whole change.
pub fn range_alignment(diff: &LinesDiff, original_lines: &[String], inner_hunk_alignment: bool) -> Vec<Alignment> {
    let mut result = Vec::new();
    for change in &diff.changes {
        let mut first = true;
        let mut last_original = change.original.start;
        let mut last_modified = change.modified.start;
        let mut emit = |original_end: usize, modified_end: usize, force: bool, result: &mut Vec<Alignment>| {
            if original_end < last_original || modified_end < last_modified {
                return;
            }
            if first {
                first = false;
            } else if !force && (original_end == last_original || modified_end == last_modified) {
                return;
            }
            let original = LineRange::new(last_original, original_end);
            let modified = LineRange::new(last_modified, modified_end);
            if original.is_empty() && modified.is_empty() {
                return;
            }
            result.push(Alignment { original, modified, changed: true });
            last_original = original_end;
            last_modified = modified_end;
        };
        if inner_hunk_alignment {
            for inner in change.inner_changes.iter().flatten() {
                if inner.original.start.column > 1 && inner.modified.start.column > 1 {
                    emit(inner.original.start.line, inner.modified.start.line, false, &mut result);
                }
                let max_column = original_lines.get(inner.original.end.line - 1).map_or(usize::MAX, |line| line.chars().count() + 1);
                if inner.original.end.column < max_column {
                    emit(inner.original.end.line, inner.modified.end.line, false, &mut result);
                }
            }
        }
        emit(change.original.end, change.modified.end, true, &mut result);
    }
    result
}

/// Which editor a view zone goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneSide {
    Original,
    Modified,
}

/// What a diff view zone shows.
#[derive(Clone, Debug, PartialEq)]
pub enum ZoneContent {
    /// `createFakeLinesDiv`: the diagonal fill of the side with fewer lines.
    Fill,
    /// The inline view's deleted lines (`line-delete`), with the change they belong to.
    Deleted { original: LineRange, change: usize },
}

/// A view zone of the diff: below `after_line` (1-based, 0 above the first line), `lines` line heights tall.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffZone {
    pub side: ZoneSide,
    pub after_line: usize,
    pub lines: usize,
    pub content: ZoneContent,
}

/// The view zones of `diffEditorViewZones.ts` for `alignments`: side by side, the diagonal fill under the shorter side
/// of each stretch; inline, the deleted lines above each change of the modified editor (the original editor is hidden).
pub fn view_zones(diff: &LinesDiff, alignments: &[Alignment], side_by_side: bool) -> Vec<DiffZone> {
    let mut zones = Vec::new();
    for alignment in alignments.iter().filter(|alignment| alignment.changed) {
        if !side_by_side {
            if !alignment.original.is_empty() {
                let change = diff.changes.iter().position(|change| change.original.start <= alignment.original.start && alignment.original.end <= change.original.end).unwrap_or(0);
                zones.push(DiffZone { side: ZoneSide::Modified, after_line: alignment.modified.start - 1, lines: alignment.original.len(), content: ZoneContent::Deleted { original: alignment.original, change } });
            }
            continue;
        }
        let (original, modified) = (alignment.original.len(), alignment.modified.len());
        if modified > original {
            zones.push(DiffZone { side: ZoneSide::Original, after_line: alignment.original.end - 1, lines: modified - original, content: ZoneContent::Fill });
        } else if original > modified {
            zones.push(DiffZone { side: ZoneSide::Modified, after_line: alignment.modified.end - 1, lines: original - modified, content: ZoneContent::Fill });
        }
    }
    zones
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn an_insertion_fills_the_original_side_and_inline_shows_nothing_deleted() {
        let (original, modified) = (lines("a\nb\nc"), lines("a\nx\ny\nb\nc"));
        let diff = diff::compute_diff(&original, &modified, DiffOptions::default());
        let alignments = range_alignment(&diff, &original, true);
        assert_eq!(alignments, vec![Alignment { original: LineRange::new(2, 2), modified: LineRange::new(2, 4), changed: true }]);
        assert_eq!(view_zones(&diff, &alignments, true), vec![DiffZone { side: ZoneSide::Original, after_line: 1, lines: 2, content: ZoneContent::Fill }]);
        assert_eq!(view_zones(&diff, &alignments, false), vec![]);
    }

    #[test]
    fn a_deletion_fills_the_modified_side_and_inline_shows_the_deleted_lines_above() {
        let (original, modified) = (lines("a\nb\nc\nd"), lines("a\nd"));
        let diff = diff::compute_diff(&original, &modified, DiffOptions::default());
        let alignments = range_alignment(&diff, &original, true);
        assert_eq!(view_zones(&diff, &alignments, true), vec![DiffZone { side: ZoneSide::Modified, after_line: 1, lines: 2, content: ZoneContent::Fill }]);
        assert_eq!(
            view_zones(&diff, &alignments, false),
            vec![DiffZone { side: ZoneSide::Modified, after_line: 1, lines: 2, content: ZoneContent::Deleted { original: LineRange::new(2, 4), change: 0 } }]
        );
    }

    #[test]
    fn inner_changes_split_a_change_into_aligned_stretches() {
        // Two changed lines, each with unchanged text after its change: one stretch per line, the fill under line 2.
        let (original, modified) = (lines("let a = 1;\nlet b = 1;\nz"), lines("let a = 2;\nlet b = 2;\nnew one\nnew two\nz"));
        let diff = diff::compute_diff(&original, &modified, DiffOptions::default());
        let alignments = range_alignment(&diff, &original, true);
        assert_eq!(alignments.first().map(|a| (a.original, a.modified)), Some((LineRange::new(1, 2), LineRange::new(1, 2))));
        assert_eq!(view_zones(&diff, &alignments, true).iter().map(|zone| (zone.side, zone.after_line, zone.lines)).collect::<Vec<_>>(), vec![(ZoneSide::Original, 2, 2)]);
    }
}
