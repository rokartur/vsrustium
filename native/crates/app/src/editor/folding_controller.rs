//! The folding commands and model updates of folding.ts (`FoldingController` and its actions) on an open editor.

use super::cursor::{self, Position, Selection};
use super::folding::{
    self, FoldRange, FoldSource, FoldingRegions, compute_ranges, next_fold_line, parent_fold_line, previous_fold_line, set_collapse_state_at_level, set_collapse_state_for_matching_lines,
    set_collapse_state_for_rest, set_collapse_state_for_type, set_collapse_state_levels_down, set_collapse_state_up, toggle_collapse_state,
};
use super::{OpenEditor, Reveal};

impl OpenEditor {
    /// `triggerFoldingModelChanged`: the indentation provider's regions merged into the model, then the
    /// cursors leave the lines it hides. While extension providers fold the model their regions stay, moved by
    /// the edits, until the workbench applies their next answer (`apply_syntax_folding`).
    pub fn refresh_folding_ranges(&mut self) {
        // folding.ts `onModelChanged`: a model too large for tokenization gets no folding model.
        if self.document.too_large_for_tokenization {
            return;
        }
        if self.syntax_folding {
            return self.adjust_selections_to_folds();
        }
        let document = &self.document;
        let regions = compute_ranges(&document.lines, document.indentation.tab_size, document.off_side, document.folding_markers.as_deref());
        self.sticky_model = super::sticky_scroll::from_folding_regions(&regions);
        let lines = self.selection_start_lines();
        self.folding.update(regions, self.document.lines.len(), Some(&lines));
        self.adjust_selections_to_folds();
    }

    /// The end of `triggerFoldingModelChanged` with a `SyntaxRangeProvider`: the providers' regions, or `None` when
    /// none answered with an array, which falls back to the indentation provider. With `editor.foldingImportsByDefault`
    /// the first regions of kind `imports` collapse.
    pub fn apply_syntax_folding(&mut self, regions: Option<FoldingRegions>, fold_imports: bool) {
        if self.document.too_large_for_tokenization {
            return;
        }
        let Some(mut regions) = regions else {
            self.syntax_folding = false;
            return self.refresh_folding_ranges();
        };
        self.syntax_folding = true;
        if fold_imports && !self.folded_imports {
            for index in 0..regions.len() {
                if regions.get(index).kind.as_deref() == Some("imports") {
                    regions.set_collapsed(index, true);
                    self.folded_imports = true;
                }
            }
        }
        self.sticky_model = super::sticky_scroll::from_folding_regions(&regions);
        let lines = self.selection_start_lines();
        self.folding.update(regions, self.document.lines.len(), Some(&lines));
        self.adjust_selections_to_folds();
    }

    /// `onHiddenRangesChanges`: a selection end on a hidden line moves to its fold line through `setSelections`,
    /// so the word highlights follow and Cursor Undo can bring the cursor back.
    fn adjust_selections_to_folds(&mut self) {
        let mut selections = self.selections.clone();
        if self.folding.adjust_selections(&self.document.lines, &mut selections) {
            self.set_selections(selections);
        }
    }

    /// `revealCursor`: regions hiding a selection's anchor line unfold.
    pub(super) fn unfold_at_hidden_cursors(&mut self) {
        let mut to_toggle = Vec::new();
        for selection in &self.selections {
            let line = selection.anchor.line + 1;
            if self.folding.is_hidden(selection.anchor.line) {
                to_toggle.extend(self.folding.all_regions_at_line(line, |range, _| range.collapsed && line > range.start));
            }
        }
        self.folding.toggle_collapse_state(to_toggle);
    }

    /// 1-based start lines of the selections (`getSelectedLines`).
    fn selection_start_lines(&self) -> Vec<usize> {
        self.selections.iter().map(|selection| selection.start().line + 1).collect()
    }

    /// Runs a folding action by its upstream id; `None` for an id that is not one. Every action reveals the
    /// primary selection's start afterwards (`FoldingAction.runEditorCommand`).
    pub fn run_folding_command(&mut self, command: &str) -> Option<Reveal> {
        let lines = self.selection_start_lines();
        let model = &mut self.folding;
        match command {
            "editor.unfold" => set_collapse_state_levels_down(model, false, 1, Some(&lines)),
            "editor.unfoldRecursively" => set_collapse_state_levels_down(model, false, usize::MAX, Some(&lines)),
            "editor.fold" => set_collapse_state_up(model, true, &lines),
            "editor.foldRecursively" => set_collapse_state_levels_down(model, true, usize::MAX, Some(&lines)),
            "editor.toggleFold" => toggle_collapse_state(model, 1, &lines),
            "editor.toggleFoldRecursively" => toggle_collapse_state(model, usize::MAX, &lines),
            "editor.foldAll" => set_collapse_state_levels_down(model, true, usize::MAX, None),
            "editor.unfoldAll" => set_collapse_state_levels_down(model, false, usize::MAX, None),
            "editor.foldAllExcept" => set_collapse_state_for_rest(model, true, &lines),
            "editor.unfoldAllExcept" => set_collapse_state_for_rest(model, false, &lines),
            "editor.foldAllBlockComments" => self.fold_block_comments(),
            "editor.foldAllMarkerRegions" => self.set_marker_regions_collapsed(true),
            "editor.unfoldAllMarkerRegions" => self.set_marker_regions_collapsed(false),
            "editor.gotoParentFold" => self.go_to_fold_line(parent_fold_line),
            "editor.gotoPreviousFold" => self.go_to_fold_line(previous_fold_line),
            "editor.gotoNextFold" => self.go_to_fold_line(next_fold_line),
            "editor.createFoldingRangeFromSelection" => self.fold_selections(),
            "editor.removeManualFoldingRanges" => {
                let selections = self.selections.clone();
                self.folding.remove_manual_ranges(&selections);
                self.refresh_folding_ranges();
            }
            "editor.toggleImportFold" => {
                let imports = (0..model.regions.len()).rev().filter(|&index| model.regions.get(index).kind.as_deref() == Some("imports")).collect();
                model.toggle_collapse_state(imports);
            }
            _ => {
                let level = command.strip_prefix("editor.foldLevel")?.parse().ok().filter(|level| (1..=7).contains(level))?;
                set_collapse_state_at_level(model, level, true, &lines);
            }
        }
        self.adjust_selections_to_folds();
        Some(Reveal::CenterIfOutside(self.selection().start()))
    }

    /// `FoldAllBlockCommentsAction`: comment regions, else the regions starting with the block comment token.
    fn fold_block_comments(&mut self) {
        if self.folding.regions.has_types() {
            return set_collapse_state_for_type(&mut self.folding, "comment", true);
        }
        let Some((token, _)) = &self.document.block_comment else { return };
        let regex = onig::Regex::new(&format!(r"^\s*{}", escape_regex(token))).expect("an escaped token is a valid pattern");
        set_collapse_state_for_matching_lines(&mut self.folding, &self.document.lines, &regex, true);
    }

    /// `FoldAllRegionsAction`/`UnfoldAllRegionsAction`: region kinds, else the lines matching the start marker.
    fn set_marker_regions_collapsed(&mut self, collapse: bool) {
        if self.folding.regions.has_types() {
            return set_collapse_state_for_type(&mut self.folding, "region", collapse);
        }
        if let Some(markers) = &self.document.folding_markers {
            set_collapse_state_for_matching_lines(&mut self.folding, &self.document.lines, &markers.start, collapse);
        }
    }

    /// `GotoParentFoldAction` and its siblings: the cursor to the start of the fold line found from the primary selection.
    fn go_to_fold_line(&mut self, find: fn(usize, &folding::FoldingModel) -> Option<usize>) {
        if let Some(line) = find(self.selection().start().line + 1, &self.folding) {
            self.set_selections(vec![Selection::caret(Position::new(line - 1, 0))]);
        }
    }

    /// `FoldRangeFromSelectionAction`: each selection over several lines becomes a collapsed manual region.
    fn fold_selections(&mut self) {
        let mut collapse = Vec::new();
        let mut caret = None;
        for selection in &self.selections {
            let (start, end) = (selection.start(), selection.end());
            let end_line = if end.column == 0 { end.line } else { end.line + 1 };
            if end_line > start.line + 1 {
                collapse.push(FoldRange { start: start.line + 1, end: end_line, kind: None, collapsed: true, source: FoldSource::UserDefined });
                // Upstream sets the selection once per range, so the last one stays.
                caret = Some(Position::new(start.line, 0));
            }
        }
        let Some(caret) = caret else { return };
        self.set_selections(vec![Selection::caret(caret)]);
        collapse.sort_by_key(|range| range.start);
        let merged = FoldingRegions::sanitize_and_merge(self.folding.regions.ranges(), &collapse, self.document.lines.len(), None);
        self.folding.update_post(FoldingRegions::new(merged));
    }

    /// A click on the folding control of `line` (`onEditorMouseUp` with the icon clicked), or on the end of a
    /// collapsed region's line: Alt toggles the regions around it, Shift the ones inside it first.
    pub fn click_fold_control(&mut self, line: usize, icon: bool, surrounding: bool, recursive: bool) -> bool {
        let model = &mut self.folding;
        let Some(region) = model.region_at_line(line + 1).filter(|&region| model.regions.get(region).start == line + 1) else {
            return false;
        };
        let collapsed = model.regions.is_collapsed(region);
        if !icon && !collapsed {
            return false;
        }
        let range = model.regions.get(region).clone();
        let mut to_toggle = Vec::new();
        if surrounding {
            let outside = model.regions_inside(None, |other, _| !other.contained_by(&range) && !range.contained_by(other));
            to_toggle = outside.iter().copied().filter(|&index| model.regions.is_collapsed(index)).collect();
            if to_toggle.is_empty() {
                to_toggle = outside;
            }
        } else {
            if recursive {
                to_toggle = model.regions_inside(Some(region), |other, _| other.collapsed == collapsed);
            }
            if collapsed || !recursive || to_toggle.is_empty() {
                to_toggle.push(region);
            }
        }
        model.toggle_collapse_state(to_toggle);
        self.adjust_selections_to_folds();
        true
    }

    /// The end of the document as the view shows it: the end of the last line not hidden.
    pub fn last_shown_position(&self) -> Position {
        let line = folding::last_shown_line(self.document.lines.len(), self.folding.hidden());
        Position::new(line, cursor::line_len(&self.document.lines, line))
    }
}

/// strings.ts `escapeRegExpCharacters`.
fn escape_regex(text: &str) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        if r"\{}*+?|^$.[]()".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_away_the_cursor_highlights_the_word_at_its_new_position() {
        let mut editor = crate::editor::edit::tests::editor("function merge() {\n    second = 1;\n    second = 2;\n}", "\n");
        editor.move_to(Position::new(1, 6), false);
        assert_eq!(editor.highlights, [(1, 4..10), (2, 4..10)]);
        editor.run_folding_command("editor.fold");
        assert_eq!(editor.selection(), Selection::caret(Position::new(0, 18)));
        assert!(editor.highlights.is_empty());
    }

    #[test]
    fn cursor_undo_brings_back_a_cursor_that_folding_moved() {
        let mut editor = crate::editor::edit::tests::editor("function merge() {\n    second = 1;\n}", "\n");
        editor.move_to(Position::new(1, 6), false);
        editor.run_folding_command("editor.fold");
        assert_eq!(editor.selection(), Selection::caret(Position::new(0, 18)));
        editor.cursor_undo();
        assert_eq!(editor.selection(), Selection::caret(Position::new(1, 6)));
        assert!(!editor.folding.is_hidden(1), "the restored cursor unfolds its region");
    }
}
