//! The merge editor's input and model: mergeEditorInput.ts (inputs and labels), model/mergeEditorModel.ts (the
//! accepted state of each modified base range, read back from the result model), model/modifiedBaseRange.ts and
//! model/mapping.ts (the base ranges both inputs change), model/textModelDiffs.ts (the result's diff from base) and
//! model/editing.ts. Drawn by `workbench/editor_view/merge_editor_view.rs`.

use diff::{DiffOptions, LineRange, LineRangeMapping};
use serde_json::{Value, json};

use crate::diff_editor::Side;
use crate::editor::{OpenEditor, Position, uri_string, uri_value};

/// The scheme of a merge editor's tab entry: the group keeps it as an editor without text, its `MergeInput` names
/// the four models it shows.
pub const MERGE_EDITOR_SCHEME: &str = "merge-editor";

/// `MergeEditorInputData`: an input's model and the labels its title bar shows.
#[derive(Clone, Debug, PartialEq)]
pub struct InputData {
    pub side: Side,
    pub title: Option<String>,
    pub description: Option<String>,
    pub detail: Option<String>,
}

/// `MergeEditorInput`: base, the two inputs, the result (output) model the group's code editor edits, and the model
/// computed from them.
pub struct MergeInput {
    pub base: Side,
    pub input1: InputData,
    pub input2: InputData,
    pub result: Side,
    pub model: Option<MergeModel>,
    /// mergeEditorInputModel.ts `resetResult`: the result had conflict markers when the editor opened.
    pub reset_pending: bool,
    /// The result model's view zones (action rows and spacers) as placed: view zone id and zone.
    pub placed_zones: Vec<(u32, Zone)>,
    /// viewModel.ts `manuallySetActiveModifiedBaseRange`, else the range at the result's cursor.
    pub active_range: Option<usize>,
    /// `getInitialResultValue`: the result's lines when the editor opened.
    pub initial_result: Vec<String>,
    /// The input `mergeEditor.toggleBetweenInputs` focused last.
    pub focused_input: Option<Input>,
}

impl MergeInput {
    pub fn new(base: Side, input1: InputData, input2: InputData, result: Side, initial_result: Vec<String>) -> Self {
        let reset_pending = initial_result.iter().any(|line| line.starts_with("<<<<<<<"));
        MergeInput { base, input1, input2, result, model: None, reset_pending, placed_zones: Vec::new(), active_range: None, initial_result, focused_input: None }
    }

    /// `MergeEditorInput.getName`.
    pub fn name(&self) -> String {
        format!("Merging: {}", self.result.name())
    }

    /// mainThreadEditorTabs.ts `_buildTabObject` of a `MergeEditorInput`: a `TextMergeInput` tab keyed by its result.
    pub fn tab_dto(&self, group: u32, entry: &OpenEditor, active: bool, dirty: bool) -> Value {
        let uri = |side: &Side| uri_value(&side.scheme, &side.path);
        json!({
            "id": format!("{group}~default-mergeEditor.Input-{} ", uri_string(&self.result.scheme, &self.result.path)),
            "label": self.name(),
            "input": { "kind": 3, "base": uri(&self.base), "input1": uri(&self.input1.side), "input2": uri(&self.input2.side), "result": uri(&self.result) },
            "editorId": "default",
            "isActive": active,
            "isPinned": false,
            "isPreview": entry.preview,
            "isDirty": dirty,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    One,
    Two,
}

impl Input {
    pub fn other(self) -> Input {
        match self {
            Input::One => Input::Two,
            Input::Two => Input::One,
        }
    }
}

/// `ModifiedBaseRangeState`: which inputs the result takes for a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Base,
    Input1,
    Input2,
    Both { first: Input, smart: bool },
    Unrecognized,
}

impl State {
    pub fn includes(self, input: Input) -> bool {
        match self {
            State::Both { .. } => true,
            State::Input1 => input == Input::One,
            State::Input2 => input == Input::Two,
            State::Base | State::Unrecognized => false,
        }
    }

    /// `withInputValue` of each state class, including `ModifiedBaseRangeStateInput1`'s
    /// `withInputValue(2, false)` that returns input 2.
    pub fn with_input(self, input: Input, value: bool, smart: bool) -> State {
        match (self, input) {
            (State::Base, Input::One) | (State::Unrecognized, Input::One) => if value { State::Input1 } else { self },
            (State::Base, Input::Two) | (State::Unrecognized, Input::Two) => if value { State::Input2 } else { self },
            (State::Input1, Input::One) | (State::Input2, Input::Two) => if value { self } else { State::Base },
            (State::Input1, Input::Two) => if value { State::Both { first: Input::One, smart } } else { State::Input2 },
            (State::Input2, Input::One) => if value { State::Both { first: Input::Two, smart } } else { State::Input2 },
            (State::Both { .. }, _) if value => self,
            (State::Both { .. }, Input::One) => State::Input2,
            (State::Both { .. }, Input::Two) => State::Input1,
        }
    }

    pub fn toggle(self, input: Input) -> State {
        self.with_input(input, !self.includes(input), true)
    }
}

/// `LineRangeEdit`: `lines` in place of `range`.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEdit {
    pub range: LineRange,
    pub lines: Vec<String>,
}

/// `ModifiedBaseRange`: a base range one or both inputs change, with the diffs from base to each input in it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModifiedBaseRange {
    pub base: LineRange,
    pub input1: LineRange,
    pub input2: LineRange,
    pub input1_diffs: Vec<LineRangeMapping>,
    pub input2_diffs: Vec<LineRangeMapping>,
}

impl ModifiedBaseRange {
    pub fn is_conflicting(&self) -> bool {
        !self.input1_diffs.is_empty() && !self.input2_diffs.is_empty()
    }

    pub fn range(&self, input: Input) -> LineRange {
        match input {
            Input::One => self.input1,
            Input::Two => self.input2,
        }
    }

    pub fn diffs(&self, input: Input) -> &[LineRangeMapping] {
        match input {
            Input::One => &self.input1_diffs,
            Input::Two => &self.input2_diffs,
        }
    }
}

/// `MergeEditorModel`: the modified base ranges of base, input1 and input2 (as of `versions`), the accepted and
/// handled state of each, and the result's diff from base (as of `result_version`).
pub struct MergeModel {
    pub versions: (u32, u32, u32),
    pub base: Vec<String>,
    pub input1: Vec<String>,
    pub input2: Vec<String>,
    pub ranges: Vec<ModifiedBaseRange>,
    pub accepted: Vec<State>,
    /// `handledInput1` and `handledInput2` of each range.
    pub handled: Vec<(bool, bool)>,
    pub result_diffs: Vec<LineRangeMapping>,
    pub result_version: Option<u32>,
    /// viewModel.ts `AttachedHistory`: the result's alternative version id after each `set_state` edit, the range
    /// and its handled states before; undoing the edit restores them, redoing it handles the range again.
    pub attached: Vec<(u32, usize, (bool, bool))>,
    pub alternative_version_id: u32,
}

/// mergeEditor's `MergeDiffComputer`: no whitespace trimming, no time limit, no moves.
const MERGE_DIFF_OPTIONS: DiffOptions = DiffOptions { ignore_trim_whitespace: false, max_computation_time_ms: 0, compute_moves: false, extend_to_subwords: false };

impl MergeModel {
    /// `ModifiedBaseRange.fromDiffs` of the base-to-input diffs; every range starts unaccepted and unhandled until
    /// the first `sync_result`.
    pub fn new(versions: (u32, u32, u32), base: Vec<String>, input1: Vec<String>, input2: Vec<String>) -> Self {
        let diffs1 = diff::compute_diff(&base, &input1, MERGE_DIFF_OPTIONS).changes;
        let diffs2 = diff::compute_diff(&base, &input2, MERGE_DIFF_OPTIONS).changes;
        let ranges: Vec<ModifiedBaseRange> = align(&diffs1, &diffs2)
            .into_iter()
            .map(|alignment| ModifiedBaseRange { base: alignment.input, input1: alignment.output1, input2: alignment.output2, input1_diffs: alignment.mappings1, input2_diffs: alignment.mappings2 })
            .collect();
        let count = ranges.len();
        MergeModel { versions, base, input1, input2, ranges, accepted: vec![State::Base; count], handled: vec![(false, false); count], result_diffs: Vec::new(), result_version: None, attached: Vec::new(), alternative_version_id: 0 }
    }

    fn lines(&self, input: Input) -> &[String] {
        match input {
            Input::One => &self.input1,
            Input::Two => &self.input2,
        }
    }

    pub fn is_handled(&self, index: usize) -> bool {
        self.handled[index].0 && self.handled[index].1
    }

    /// `unhandledConflictsCount`: every modified base range counts, conflicting or not.
    pub fn unhandled_count(&self) -> usize {
        (0..self.ranges.len()).filter(|&index| !self.is_handled(index)).count()
    }

    /// Recomputes the result's diff from base when the result changed since the last call; `derive_states` (a text
    /// change, not one of `set_state`'s edits) reads the accepted states back from it (`updateBaseRangeAcceptedState`),
    /// and the first time the handled states too.
    pub fn sync_result(&mut self, result: &[String], version: u32, derive_states: bool) {
        if self.result_version == Some(version) {
            return;
        }
        let first_run = self.result_version.is_none();
        self.result_version = Some(version);
        self.result_diffs = diff::compute_diff(&self.base, result, MERGE_DIFF_OPTIONS).changes;
        if !derive_states {
            return;
        }
        for index in 0..self.ranges.len() {
            let touching: Vec<&LineRangeMapping> = self.result_diffs.iter().filter(|diff| diff.original.intersects_or_touches(&self.ranges[index].base)).collect();
            self.accepted[index] = self.compute_state(index, &touching, result);
            if first_run {
                let handled = !matches!(self.accepted[index], State::Base | State::Unrecognized);
                self.handled[index] = (handled, handled);
            }
        }
    }

    /// `computeState`: the state whose edit the result's lines in the range equal.
    fn compute_state(&self, index: usize, touching: &[&LineRangeMapping], result: &[String]) -> State {
        if touching.is_empty() {
            return State::Base;
        }
        let range = &self.ranges[index];
        let edits: Vec<LineEdit> = touching.iter().map(|diff| line_edit(diff, result)).collect();
        let agrees = |input: Input| range.diffs(input).iter().map(|diff| line_edit(diff, self.lines(input))).eq(edits.iter().cloned());
        if agrees(Input::One) {
            return State::Input1;
        }
        if agrees(Input::Two) {
            return State::Input2;
        }
        let result_range = self.result_line_range(range.base);
        let existing = &result[result_range.start - 1..(result_range.end - 1).min(result.len())];
        let combinations = [(Input::One, true), (Input::Two, true), (Input::One, false), (Input::Two, false)];
        for (first, smart) in combinations {
            let state = State::Both { first, smart };
            if let (Some(edit), _) = self.edit_for_base(index, state)
                && edit.lines == existing
            {
                return state;
            }
        }
        State::Unrecognized
    }

    /// `getEditForBase`: the base edit of a state and the state it really gives.
    pub fn edit_for_base(&self, index: usize, state: State) -> (Option<LineEdit>, State) {
        let range = &self.ranges[index];
        let mut included = Vec::new();
        for input in [Input::One, Input::Two] {
            if state.includes(input)
                && let Some(combined) = join(range.diffs(input))
            {
                included.push((combined, input));
            }
        }
        match &included[..] {
            [] => (None, State::Base),
            [(combined, input)] => (Some(line_edit(combined, self.lines(*input))), State::Base.with_input(*input, true, false)),
            _ => {
                let State::Both { first, smart } = state else { unreachable!("only both states include both inputs") };
                let combined = if smart { self.smart_combine(index, first) } else { Some(self.dumb_combine(index, first)) };
                match combined {
                    Some(edit) => (Some(edit), state),
                    None => {
                        let other = first.other();
                        let (combined, _) = included.iter().find(|(_, input)| *input == other).expect("both inputs included");
                        (Some(line_edit(combined, self.lines(other))), State::Base.with_input(other, true, false))
                    }
                }
            }
        }
    }

    /// `canBeCombined`.
    pub fn can_be_combined(&self, index: usize) -> bool {
        self.smart_combine(index, Input::One).is_some()
    }

    /// `isOrderRelevant`.
    pub fn is_order_relevant(&self, index: usize) -> bool {
        match (self.smart_combine(index, Input::One), self.smart_combine(index, Input::Two)) {
            (Some(one), Some(two)) => one != two,
            _ => false,
        }
    }

    /// `smartCombineInputs`: both inputs' character changes applied to base, `first`'s first where they start
    /// together; `None` when they overlap.
    fn smart_combine(&self, index: usize, first: Input) -> Option<LineEdit> {
        let range = &self.ranges[index];
        let mut changes = Vec::new();
        for input in [Input::One, Input::Two] {
            for diff in range.diffs(input) {
                for inner in inner_changes(diff) {
                    changes.push((inner, input));
                }
            }
        }
        changes.sort_by_key(|(inner, input)| {
            let (start, end) = (inner.original.start, inner.original.end);
            (start.line, start.column, end.line, end.column, *input != first)
        });
        let edits: Vec<(diff::Range, String)> = changes.into_iter().map(|(inner, input)| (inner.original, text_in_range(self.lines(input), inner.modified.start, inner.modified.end))).collect();
        edits_to_line_edit(range.base, &edits, &self.base)
    }

    /// `dumbCombineInputs`: `first`'s lines, then the other input's.
    fn dumb_combine(&self, index: usize, first: Input) -> LineEdit {
        let range = &self.ranges[index];
        let mut lines = range_lines(self.lines(first), range.range(first)).to_vec();
        lines.extend_from_slice(range_lines(self.lines(first.other()), range.range(first.other())));
        LineEdit { range: range.base, lines }
    }

    /// `TextModelDiffs.getResultLineRange`: where a base range is in the result.
    pub fn result_line_range(&self, base: LineRange) -> LineRange {
        let start = match self.result_line(base.start) {
            Ok(line) => line,
            Err(diff) => diff.modified.start,
        };
        let end = match self.result_line(base.end) {
            Ok(line) => line,
            Err(diff) => diff.modified.end,
        };
        LineRange::new(start, end.max(start))
    }

    /// `getResultLine`: the result line of a base line, or the result diff it is in or ends at.
    fn result_line(&self, line: usize) -> Result<usize, &LineRangeMapping> {
        let mut offset = 0isize;
        for diff in &self.result_diffs {
            if diff.original.contains(line) || diff.original.end == line {
                return Err(diff);
            } else if diff.original.end < line {
                offset = diff.modified.end as isize - diff.original.end as isize;
            } else {
                break;
            }
        }
        Ok(line.saturating_add_signed(offset))
    }

    /// `MergeEditorModel.setState`: the result diffs touching the range go back to base, then the state's edit
    /// applies; returns that as one edit of the result's lines. The range is handled after.
    pub fn set_state(&mut self, index: usize, state: State) -> LineEdit {
        let base_range = self.ranges[index].base;
        let (edit, effective) = self.edit_for_base(index, state);
        let mut reverted = base_range;
        for diff in self.result_diffs.iter().filter(|diff| diff.original.intersects_or_touches(&base_range)) {
            reverted = reverted.join(&diff.original);
        }
        let edit = edit.unwrap_or_else(|| LineEdit { range: base_range, lines: range_lines(&self.base, base_range).to_vec() });
        let mut lines = self.base[reverted.start - 1..edit.range.start - 1].to_vec();
        lines.extend(edit.lines);
        lines.extend_from_slice(&self.base[edit.range.end - 1..reverted.end - 1]);
        self.accepted[index] = effective;
        self.handled[index] = (true, true);
        LineEdit { range: self.result_line_range(reverted), lines }
    }

    /// `pushAttachedHistoryElement` for the `set_state` edit that left the result at `alternative_version_id`.
    pub fn attach(&mut self, alternative_version_id: u32, index: usize, handled_before: (bool, bool)) {
        self.attached.push((alternative_version_id, index, handled_before));
        self.alternative_version_id = alternative_version_id;
    }

    /// `AttachedHistory`'s content change listener: an undo below an attached edit restores its range's handled
    /// states, a redo past it handles the range again, a new edit drops the ones it can no longer redo. A new edit's
    /// alternative version id is above every recorded one, a redo's is one of them.
    pub fn follow_history(&mut self, alternative_version_id: u32) {
        let previous = std::mem::replace(&mut self.alternative_version_id, alternative_version_id);
        if alternative_version_id < previous {
            for &(id, index, before) in self.attached.iter().rev() {
                if alternative_version_id < id && id <= previous {
                    self.handled[index] = before;
                }
            }
        } else if alternative_version_id > previous {
            let newest = self.attached.iter().map(|item| item.0).max().unwrap_or(0);
            if alternative_version_id > newest {
                self.attached.retain(|item| item.0 <= previous);
                return;
            }
            for &(id, index, _) in &self.attached {
                if previous < id && id <= alternative_version_id {
                    self.handled[index] = (true, true);
                }
            }
        }
    }

    /// conflictActions.ts: a range shows actions when it conflicts, is unhandled or non-conflicting changes show.
    pub fn shows_actions(&self, index: usize, show_non_conflicting: bool) -> bool {
        self.ranges[index].is_conflicting() || show_non_conflicting || !self.is_handled(index)
    }

    /// viewZones.ts `computeViewZones`: the action zones above each range and the spacers that line the inputs
    /// (and base, and result in the column layout) up; `inputs_empty` tells a range without input actions.
    pub fn view_zones(&self, options: ZoneOptions, inputs_empty: impl Fn(usize) -> bool) -> ViewZones {
        let mut zones = ViewZones::default();
        let mut added = [0isize; 4];
        let delta = |diff: Option<&LineRangeMapping>| diff.map_or(0, |diff| diff.modified.end as isize - diff.original.end as isize);
        let mut last_range: Option<usize> = None;
        let mut last_result_diff: Option<&LineRangeMapping> = None;
        for (left, rights) in self.join_result_diffs() {
            if let Some(index) = left.filter(|&index| self.shows_actions(index, options.show_non_conflicting)) {
                let range = &self.ranges[index];
                if options.align_result || !inputs_empty(index) {
                    zones.input1.push(Zone { after_line: range.input1.start - 1, kind: ZoneKind::Actions(index) });
                    zones.input2.push(Zone { after_line: range.input2.start - 1, kind: ZoneKind::Actions(index) });
                    if options.align_base {
                        zones.base.push(Zone { after_line: range.base.start - 1, kind: ZoneKind::Placeholder });
                    }
                }
                let after_line = (range.base.start as isize + delta(last_result_diff) - 1).max(0) as usize;
                zones.result.push(Zone { after_line, kind: ZoneKind::Actions(index) });
            }
            if let Some(last) = rights.last() {
                last_result_diff = Some(last);
            }
            let alignments: Vec<[Option<isize>; 4]> = match left {
                Some(index) => {
                    last_range = Some(index);
                    let mut alignments: Vec<[Option<isize>; 4]> = self.alignments(index).into_iter().map(|(input1, base, input2)| [input1, Some(base), input2, None]).collect();
                    // viewZones.ts "This is a total hack": the range's end lines up with its end in the result.
                    alignments.last_mut().expect("the final alignment")[3] = Some(self.ranges[index].base.end as isize + delta(last_result_diff));
                    alignments
                }
                None => {
                    let last = rights.last().expect("a row without range has result diffs");
                    let base_end = last.original.end as isize;
                    let shift = |input: Input| last_range.map_or(0, |index| self.ranges[index].range(input).end as isize - self.ranges[index].base.end as isize);
                    vec![[Some(base_end + shift(Input::One)), Some(base_end), Some(base_end + shift(Input::Two)), Some(last.modified.end as isize)]]
                }
            };
            for alignment in alignments {
                let [input1, base, input2, result] = alignment;
                if !options.align_base && (input1.is_none() || input2.is_none()) {
                    continue;
                }
                let base = base.expect("every alignment has a base line");
                let shifted = |line: Option<isize>, side: usize| line.map_or(-1, |line| line + added[side]);
                let (input1_, input2_, base_, result_) = (shifted(input1, 0), shifted(input2, 1), base + added[2], shifted(result, 3));
                let max = [if options.align_base { base_ } else { 0 }, input1_, input2_, if options.align_result { result_ } else { 0 }].into_iter().max().expect("four values");
                let mut spacer = |line: isize, shifted: isize, side: usize, zones: &mut Vec<Zone>| {
                    let lines = max - shifted;
                    if lines > 0 {
                        zones.push(Zone { after_line: (line - 1).max(0) as usize, kind: ZoneKind::Spacer(lines as usize) });
                        added[side] += lines;
                    }
                };
                if let Some(input1) = input1 {
                    spacer(input1, input1_, 0, &mut zones.input1);
                }
                if let Some(input2) = input2 {
                    spacer(input2, input2_, 1, &mut zones.input2);
                }
                if options.align_base {
                    spacer(base, base_, 2, &mut zones.base);
                }
                if let (true, Some(result)) = (options.align_result, result) {
                    spacer(result, result_, 3, &mut zones.result);
                }
            }
        }
        zones
    }

    /// utils.ts `join` of the ranges and the result diffs touching them: result diffs before a range form a row of
    /// their own; those after the last range are left out, as upstream.
    fn join_result_diffs(&self) -> Vec<(Option<usize>, Vec<&LineRangeMapping>)> {
        let mut rows = Vec::new();
        let mut diffs = self.result_diffs.iter().peekable();
        for (index, range) in self.ranges.iter().enumerate() {
            let mut skipped = Vec::new();
            while let Some(diff) = diffs.next_if(|diff| !range.base.intersects_or_touches(&diff.original) && diff.original.start < range.base.start) {
                skipped.push(diff);
            }
            if !skipped.is_empty() {
                rows.push((None, skipped));
            }
            let mut touching = Vec::new();
            while let Some(diff) = diffs.next_if(|diff| range.base.intersects_or_touches(&diff.original)) {
                touching.push(diff);
            }
            rows.push((Some(index), touching));
        }
        rows
    }

    /// lineAlignment.ts `getAlignments`: (input 1, base, input 2) lines of a range that show at one height; an
    /// input is `None` where only the other one lines up with base.
    pub fn alignments(&self, index: usize) -> Vec<(Option<isize>, isize, Option<isize>)> {
        let range = &self.ranges[index];
        let exclusive = |range: LineRange| ((range.start, 1), (range.end, 1));
        let equal1 = equal_range_mappings(range.diffs(Input::One), exclusive(range.base), exclusive(range.input1));
        let equal2 = equal_range_mappings(range.diffs(Input::Two), exclusive(range.base), exclusive(range.input2));
        let line = |value: usize| value as isize;
        let mut result: Vec<(Option<isize>, isize, Option<isize>)> = vec![(Some(line(range.input1.start) - 1), line(range.base.start) - 1, Some(line(range.input2.start) - 1))];
        let shares = |a: &(Option<isize>, isize, Option<isize>), b: &(Option<isize>, isize, Option<isize>)| (a.0.is_some() && a.0 == b.0) || a.1 == b.1 || (a.2.is_some() && a.2 == b.2);
        let full = |a: &(Option<isize>, isize, Option<isize>)| a.0.is_some() && a.2.is_some();
        for common in split_up_common(&equal1, &equal2) {
            let alignment = (common.output1.map(|pos| line(pos.0)), line(common.input.0), common.output2.map(|pos| line(pos.0)));
            let should_add = if full(&alignment) {
                let is_new = !result.iter().any(|existing| full(existing) && shares(existing, &alignment));
                if is_new {
                    result.retain(|existing| !shares(existing, &alignment));
                }
                is_new
            } else {
                !result.iter().any(|existing| shares(existing, &alignment))
            };
            if should_add {
                result.push(alignment);
            } else if common.length > (1, 0) {
                result.push((common.output1.map(|pos| line(pos.0) + 1), line(common.input.0) + 1, common.output2.map(|pos| line(pos.0) + 1)));
            }
        }
        let last = (Some(line(range.input1.end)), line(range.base.end), Some(line(range.input2.end)));
        result.retain(|existing| existing.0 != last.0 && existing.1 != last.1 && existing.2 != last.2);
        result.push(last);
        result
    }

    /// `setInputHandled` (Ignore).
    pub fn set_input_handled(&mut self, index: usize, input: Input) {
        match input {
            Input::One => self.handled[index].0 = true,
            Input::Two => self.handled[index].1 = true,
        }
    }

    /// `MergeEditorModel.reset`: every range takes the only input that changes it (input 1 for equal changes), a
    /// conflict stays base and unhandled; returns the edit replacing the whole result (`computeAutoMergedResult`).
    pub fn reset(&mut self, result_line_count: usize) -> LineEdit {
        let mut lines = Vec::new();
        let mut base_start = 1;
        for index in 0..self.ranges.len() {
            let range = &self.ranges[index];
            lines.extend_from_slice(&self.base[base_start - 1..range.base.start - 1]);
            base_start = range.base.end;
            let (state, source, source_range) = if range.input1_diffs.is_empty() {
                (State::Input2, &self.input2, range.input2)
            } else if range.input2_diffs.is_empty() || self.is_equal_change(index) {
                (State::Input1, &self.input1, range.input1)
            } else {
                (State::Base, &self.base, range.base)
            };
            lines.extend_from_slice(range_lines(source, source_range));
            let handled = state != State::Base;
            self.accepted[index] = state;
            self.handled[index] = (handled, handled);
        }
        lines.extend_from_slice(&self.base[base_start - 1..]);
        LineEdit { range: LineRange::new(1, result_line_count + 1), lines }
    }

    /// `isEqualChange`: both inputs make the same line edits.
    fn is_equal_change(&self, index: usize) -> bool {
        let range = &self.ranges[index];
        let edits = |input: Input| range.diffs(input).iter().map(|diff| line_edit(diff, self.lines(input))).collect::<Vec<_>>();
        edits(Input::One) == edits(Input::Two)
    }

    /// `DocumentLineRangeMap.project` of result to base, then base to `input`: the input line shown beside a
    /// result line, for the panes' scroll synchronization.
    pub fn input_line_for_result_line(&self, input: Input, result_line: usize) -> usize {
        let base_line = self.base_line_for_result_line(result_line);
        let diffs = self.ranges.iter().flat_map(|range| range.diffs(input));
        project_line(diffs.map(|diff| (diff.original, diff.modified)), base_line)
    }

    /// `resultBaseMapping.project`: the base line shown beside a result line.
    pub fn base_line_for_result_line(&self, result_line: usize) -> usize {
        project_line(self.result_diffs.iter().map(|diff| (diff.modified, diff.original)), result_line)
    }
}

/// The layout options `computeViewZones` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct ZoneOptions {
    pub align_result: bool,
    pub align_base: bool,
    pub show_non_conflicting: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoneKind {
    /// `CommandViewZone`: the action row of a range.
    Actions(usize),
    /// `Spacer`: lines of diagonal fill.
    Spacer(usize),
    /// `Placeholder`: 16px in base for the inputs' action rows.
    Placeholder,
}

/// A view zone below 1-based `after_line` (0: above the first line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub after_line: usize,
    pub kind: ZoneKind,
}

/// `MergeEditorViewZones`.
#[derive(Debug, Default, PartialEq)]
pub struct ViewZones {
    pub input1: Vec<Zone>,
    pub input2: Vec<Zone>,
    pub base: Vec<Zone>,
    pub result: Vec<Zone>,
}

/// mergeEditor.ts `IMergeEditorLayout`, kept in profile storage under `mergeEditor/layout`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub columns: bool,
    pub show_base: bool,
    pub show_base_at_top: bool,
}

pub const LAYOUT_KEY: &str = "mergeEditor/layout";
pub const SHOW_NON_CONFLICTING_KEY: &str = "mergeEditor/showNonConflictingChanges";

impl Layout {
    /// `MergeEditorLayoutStore`: "mixed" or "columns" from older versions, else the JSON layout; mixed by default.
    pub fn parse(stored: Option<&str>) -> Layout {
        let default = Layout { columns: false, show_base: false, show_base_at_top: true };
        match stored {
            None | Some("mixed") => default,
            Some("columns") => Layout { columns: true, ..default },
            Some(json) => {
                let Ok(value) = serde_json::from_str::<Value>(json) else { return default };
                Layout {
                    columns: value["kind"] == "columns",
                    show_base: value["showBase"] == true,
                    show_base_at_top: value["showBaseAtTop"] != false,
                }
            }
        }
    }

    pub fn to_json(self) -> String {
        json!({ "kind": if self.columns { "columns" } else { "mixed" }, "showBase": self.show_base, "showBaseAtTop": self.show_base_at_top }).to_string()
    }
}

type Pos = (usize, usize);

/// mapping.ts `RangeMapping` of 1-based (line, column) positions: `[input start, input end)` to output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PosMapping {
    pub input: (Pos, Pos),
    pub output: (Pos, Pos),
}

/// mapping.ts `DocumentRangeMap`: a monotonous mapping of ranges in one document to another, with holes.
pub struct DocumentRangeMap(pub Vec<PosMapping>);

impl DocumentRangeMap {
    /// The character changes of line mappings (`rangeMappings`, whole lines without them).
    pub fn of(diffs: &[LineRangeMapping]) -> Self {
        let position = |position: diff::Position| (position.line, position.column);
        DocumentRangeMap(diffs.iter().flat_map(inner_changes).map(|change| PosMapping { input: (position(change.original.start), position(change.original.end)), output: (position(change.modified.start), position(change.modified.end)) }).collect())
    }

    pub fn reverse(&self) -> Self {
        DocumentRangeMap(self.0.iter().map(|mapping| PosMapping { input: mapping.output, output: mapping.input }).collect())
    }

    /// `project`: the mapping containing `position`, else the empty one it moves to past the last mapping before it.
    pub fn project(&self, position: Pos) -> PosMapping {
        let Some(last) = self.0.iter().rev().find(|mapping| mapping.input.0 <= position) else { return PosMapping { input: (position, position), output: (position, position) } };
        if position < last.input.1 {
            return *last;
        }
        let output = add_length(last.output.1, length_between(last.input.1, position));
        PosMapping { input: (position, position), output: (output, output) }
    }

    /// `projectRange`: the output range of a range's projected start and end.
    pub fn project_range(&self, range: (Pos, Pos)) -> (Pos, Pos) {
        let (start, end) = (self.project(range.0), self.project(range.1));
        (start.output.0.min(end.output.0), start.output.1.max(end.output.1))
    }
}

/// rangeUtils.ts `lengthBetweenPositions`, as (lines, columns).
fn length_between(start: Pos, end: Pos) -> (usize, usize) {
    if start.0 == end.0 { (0, end.1 - start.1) } else { (end.0 - start.0, end.1 - 1) }
}

/// rangeUtils.ts `addLength`.
fn add_length(position: Pos, length: (usize, usize)) -> Pos {
    if length.0 == 0 { (position.0, position.1 + length.1) } else { (position.0 + length.0, length.1 + 1) }
}

/// lineAlignment.ts `toEqualRangeMappings`: the unchanged stretches between a side's character changes, as (base
/// start, base end, input start).
fn equal_range_mappings(diffs: &[LineRangeMapping], base: (Pos, Pos), input: (Pos, Pos)) -> Vec<(Pos, Pos, Pos)> {
    let mut result = Vec::new();
    let (mut base_start, mut input_start) = (base.0, input.0);
    let position = |position: diff::Position| (position.line, position.column);
    for change in diffs.iter().flat_map(inner_changes) {
        if base_start != position(change.original.start) {
            result.push((base_start, position(change.original.start), input_start));
        }
        base_start = position(change.original.end);
        input_start = position(change.modified.end);
    }
    if base_start != base.1 {
        result.push((base_start, base.1, input_start));
    }
    result
}

/// lineAlignment.ts `CommonRangeMapping`.
struct CommonRange {
    input: Pos,
    length: (usize, usize),
    output1: Option<Pos>,
    output2: Option<Pos>,
}

/// lineAlignment.ts `splitUpCommonEqualRangeMappings`: the base stretches unchanged in one or both inputs.
fn split_up_common(equal1: &[(Pos, Pos, Pos)], equal2: &[(Pos, Pos, Pos)]) -> Vec<CommonRange> {
    let mut events: Vec<(usize, bool, Pos, Pos)> = Vec::new();
    for (side, mappings) in [(0, equal1), (1, equal2)] {
        for &(base_start, base_end, output_start) in mappings {
            let output_end = add_length(output_start, length_between(base_start, base_end));
            events.push((side, true, base_start, output_start));
            events.push((side, false, base_end, output_end));
        }
    }
    events.sort_by_key(|event| event.2);
    let mut result = Vec::new();
    let mut starts: [Option<Pos>; 2] = [None, None];
    let mut last_input: Option<Pos> = None;
    for (side, start, input, output) in events {
        if let Some(last) = last_input.filter(|_| starts.iter().any(Option::is_some)) {
            let length = length_between(last, input);
            if length != (0, 0) {
                result.push(CommonRange { input: last, length, output1: starts[0], output2: starts[1] });
                for start in starts.iter_mut().flatten() {
                    *start = add_length(*start, length);
                }
            }
        }
        starts[side] = start.then_some(output);
        last_input = Some(input);
    }
    result
}

/// `DocumentLineRangeMap.project`, the start of the line's mapped range.
fn project_line(mappings: impl Iterator<Item = (LineRange, LineRange)>, line: usize) -> usize {
    let mut projected = line;
    for (input, output) in mappings {
        if input.start > line {
            break;
        }
        projected = if input.contains(line) { output.start } else { (line + output.end).saturating_sub(input.end) };
    }
    projected
}

/// `DetailedLineRangeMapping.getLineEdit`: the mapping's output lines in place of its input range.
fn line_edit(mapping: &LineRangeMapping, output: &[String]) -> LineEdit {
    LineEdit { range: mapping.original, lines: range_lines(output, mapping.modified).to_vec() }
}

fn range_lines(lines: &[String], range: LineRange) -> &[String] {
    &lines[(range.start - 1).min(lines.len())..(range.end - 1).min(lines.len())]
}

/// `DetailedLineRangeMapping.join` of every mapping; the joined mapping has no character changes.
fn join(mappings: &[LineRangeMapping]) -> Option<LineRangeMapping> {
    let (first, rest) = mappings.split_first()?;
    let mut joined = LineRangeMapping::new(first.original, first.modified);
    for mapping in rest {
        joined = LineRangeMapping::new(joined.original.join(&mapping.original), joined.modified.join(&mapping.modified));
    }
    Some(joined)
}

/// `rangeMappings`: the character changes, else the whole lines as one.
fn inner_changes(mapping: &LineRangeMapping) -> Vec<diff::RangeMapping> {
    match &mapping.inner_changes {
        Some(changes) => changes.clone(),
        None => vec![diff::RangeMapping { original: diff::Range::new(mapping.original.start, 1, mapping.original.end, 1), modified: diff::Range::new(mapping.modified.start, 1, mapping.modified.end, 1) }],
    }
}

/// `MappingAlignment`: base ranges with the output ranges and mappings of both sides.
struct Alignment {
    input: LineRange,
    output1: LineRange,
    mappings1: Vec<LineRangeMapping>,
    output2: LineRange,
    mappings2: Vec<LineRangeMapping>,
}

/// `MappingAlignment.compute`: the mappings of both sides grouped where their input ranges touch.
fn align(diffs1: &[LineRangeMapping], diffs2: &[LineRangeMapping]) -> Vec<Alignment> {
    let mut combined: Vec<(usize, &LineRangeMapping)> = diffs1.iter().map(|diff| (0, diff)).chain(diffs2.iter().map(|diff| (1, diff))).collect();
    combined.sort_by_key(|(_, diff)| diff.original.start);
    let mut current: [Vec<LineRangeMapping>; 2] = [Vec::new(), Vec::new()];
    let mut delta = [0isize; 2];
    let mut current_input: Option<LineRange> = None;
    let mut alignments = Vec::new();
    let push = |input: LineRange, current: &mut [Vec<LineRangeMapping>; 2], delta: [isize; 2], alignments: &mut Vec<Alignment>| {
        let output = |side: usize| match join(&current[side]) {
            Some(mapping) => extend_input_range(&mapping, input),
            None => LineRange::new(input.start.saturating_add_signed(delta[side]), input.end.saturating_add_signed(delta[side])),
        };
        let (output1, output2) = (output(0), output(1));
        alignments.push(Alignment { input, output1, mappings1: std::mem::take(&mut current[0]), output2, mappings2: std::mem::take(&mut current[1]) });
    };
    for (side, diff) in combined {
        if let Some(input) = current_input.filter(|input| !input.intersects_or_touches(&diff.original)) {
            push(input, &mut current, delta, &mut alignments);
            current_input = None;
        }
        delta[side] = diff.modified.end as isize - diff.original.end as isize;
        current_input = Some(current_input.map_or(diff.original, |input| input.join(&diff.original)));
        current[side].push(diff.clone());
    }
    if let Some(input) = current_input {
        push(input, &mut current, delta, &mut alignments);
    }
    alignments
}

/// `LineRangeMapping.extendInputRange`: the output range of `mapping` grown by what `extended` adds to its input.
fn extend_input_range(mapping: &LineRangeMapping, extended: LineRange) -> LineRange {
    let start_delta = mapping.original.start - extended.start;
    let end_delta = extended.end - mapping.original.end;
    LineRange::new(mapping.modified.start - start_delta, mapping.modified.end + end_delta)
}

/// `editsToLineRangeEdit`: character edits of base turned into a line edit of `range`; `None` when they overlap or
/// change the line breaks around the range.
fn edits_to_line_edit(range: LineRange, edits: &[(diff::Range, String)], base: &[String]) -> Option<LineEdit> {
    let starts_line_before = range.start > 1;
    let mut current = if starts_line_before { diff::Position { line: range.start - 1, column: max_column(base, range.start - 1) } } else { diff::Position { line: range.start, column: 1 } };
    let mut text = String::new();
    for (edit_range, new_text) in edits {
        let start = edit_range.start;
        if (current.line, current.column) > (start.line, start.column) {
            return None;
        }
        text.push_str(&text_in_range(base, current, start));
        // getValueInRange stops at the end of the model, before the line break of the line past it.
        if start.line > base.len() {
            text.push('\n');
        }
        text.push_str(new_text);
        current = edit_range.end;
    }
    let ends_line_after = range.end <= base.len();
    let end = if ends_line_after { diff::Position { line: range.end, column: 1 } } else { diff::Position { line: range.end - 1, column: usize::MAX } };
    text.push_str(&text_in_range(base, current, end));
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if starts_line_before {
        if !lines[0].is_empty() {
            return None;
        }
        lines.remove(0);
    }
    if ends_line_after {
        if lines.last().is_some_and(|line| !line.is_empty()) {
            return None;
        }
        lines.pop();
    }
    Some(LineEdit { range, lines })
}

fn max_column(lines: &[String], line: usize) -> usize {
    lines[line - 1].chars().count() + 1
}

/// `validatePosition`: a line past the model is the end of its last line, a column past its line the line's end.
fn validate(lines: &[String], position: diff::Position) -> (usize, usize) {
    if position.line > lines.len() {
        return (lines.len(), max_column(lines, lines.len()));
    }
    let line = position.line.max(1);
    (line, position.column.clamp(1, max_column(lines, line)))
}

/// `getValueInRange` with "\n" line breaks; columns count characters.
fn text_in_range(lines: &[String], start: diff::Position, end: diff::Position) -> String {
    let (start, end) = (validate(lines, start), validate(lines, end));
    if start >= end {
        return String::new();
    }
    let mut text = String::new();
    for line in start.0..=end.0 {
        let chars = lines[line - 1].chars();
        let from = if line == start.0 { start.1 - 1 } else { 0 };
        match line == end.0 {
            true => text.extend(chars.skip(from).take(end.1 - 1 - from)),
            false => {
                text.extend(chars.skip(from));
                text.push('\n');
            }
        }
    }
    text
}

/// `getValueInRange` of 1-based (line, column) positions.
pub fn text_of(lines: &[String], range: (Pos, Pos)) -> String {
    let position = |(line, column): Pos| diff::Position { line, column };
    text_in_range(lines, position(range.0), position(range.1))
}

/// editing.ts `LineEdits.toEdits`: the text edit of a line edit on a model of `lines`, 0-based.
pub fn text_edit(edit: &LineEdit, lines: &[String], eol: &str) -> (Position, Position, String) {
    let count = lines.len();
    let line_end = |line: usize| Position::new(line, lines[line].chars().count());
    if edit.range.end <= count {
        let text = edit.lines.iter().map(|line| format!("{line}{eol}")).collect();
        return (Position::new(edit.range.start - 1, 0), Position::new(edit.range.end - 1, 0), text);
    }
    if edit.range.start == 1 {
        return (Position::new(0, 0), line_end(count - 1), edit.lines.join(eol));
    }
    let text = edit.lines.iter().map(|line| format!("{eol}{line}")).collect();
    (line_end(edit.range.start - 2), line_end(count - 1), text)
}

#[cfg(test)]
mod tests;
