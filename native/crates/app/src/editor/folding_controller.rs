//! The folding commands and model updates of folding.ts (`FoldingController` and its actions) on an open editor.

use super::cursor::{self, Position, Selection};
use super::folding::{
    self, FoldRange, FoldSource, FoldingRegions, compute_ranges, next_fold_line, parent_fold_line, previous_fold_line, set_collapse_state_at_level, set_collapse_state_for_matching_lines,
    set_collapse_state_for_rest, set_collapse_state_for_type, set_collapse_state_levels_down, set_collapse_state_up, toggle_collapse_state,
};
use super::{OpenEditor, Reveal};

impl OpenEditor {
    /// `triggerFoldingModelChanged`: the indentation provider's regions merged into the model, then the
    /// cursors leave the lines it hides.
    pub fn refresh_folding_ranges(&mut self) {
        let document = &self.document;
        let regions = compute_ranges(&document.lines, document.indentation.tab_size, document.off_side, document.folding_markers.as_deref());
        let lines = self.selection_start_lines();
        self.folding.update(regions, self.document.lines.len(), Some(&lines));
        self.adjust_selections_to_folds();
    }

    /// `onHiddenRangesChanges`: a selection end on a hidden line moves to its fold line.
    fn adjust_selections_to_folds(&mut self) {
        let mut selections = self.selections.clone();
        if self.folding.adjust_selections(&self.document.lines, &mut selections) {
            self.selections = selections;
            cursor::normalize(&mut self.selections, &mut self.last_added);
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
            "editor.gotoParentFold" => return Some(self.go_to_fold_line(parent_fold_line)),
            "editor.gotoPreviousFold" => return Some(self.go_to_fold_line(previous_fold_line)),
            "editor.gotoNextFold" => return Some(self.go_to_fold_line(next_fold_line)),
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
        Some(Reveal::Position(self.selection().start()))
    }

    /// `FoldAllBlockCommentsAction`: comment regions, else the regions starting with the block comment token.
    fn fold_block_comments(&mut self) {
        if self.folding.regions.has_types() {
            return set_collapse_state_for_type(&mut self.folding, "comment", true);
        }
        let Some(token) = &self.document.block_comment_start else { return };
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
    fn go_to_fold_line(&mut self, find: fn(usize, &folding::FoldingModel) -> Option<usize>) -> Reveal {
        let Some(line) = find(self.selection().start().line + 1, &self.folding) else {
            return Reveal::Nothing;
        };
        let position = Position::new(line - 1, 0);
        self.set_selections(vec![Selection::caret(position)]);
        Reveal::Position(position)
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
