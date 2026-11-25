//! Word wrap (`editor.wordWrap`): monospaceLineBreaksComputer.ts breaks each model line into view lines, and
//! viewModelLines.ts `ViewModelLinesFromProjectedModel` maps model positions onto the wrapped and folded view.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use super::cursor::{self, Position, Selection};
use super::{OpenEditor, folding, minimap::is_full_width_character, visible_column};

/// The `wordWrapBreakBeforeCharacters` and `wordWrapBreakAfterCharacters` defaults (editorOptions.ts); they have no
/// settings schema, so no setting changes them.
pub const BREAK_BEFORE: &str = "([{\u{2018}\u{201C}\u{3008}\u{300A}\u{300C}\u{300E}\u{3010}\u{3014}\u{FF08}\u{FF3B}\u{FF5B}\u{FF62}\u{A3}\u{A5}\u{FF04}\u{FFE1}\u{FFE5}+\u{FF0B}";
pub const BREAK_AFTER: &str = " \t})]?|/&.,;\u{A2}\u{B0}\u{2032}\u{2033}\u{2030}\u{2103}\u{3001}\u{3002}\u{FF61}\u{FF64}\u{FFE0}\u{FF0C}\u{FF0E}\u{FF1A}\u{FF1B}\u{FF1F}\u{FF01}\u{FF05}\u{30FB}\u{FF65}\u{309D}\u{309E}\u{30FD}\u{30FE}\u{30FC}\u{30A1}\u{30A3}\u{30A5}\u{30A7}\u{30A9}\u{30C3}\u{30E3}\u{30E5}\u{30E7}\u{30EE}\u{30F5}\u{30F6}\u{3041}\u{3043}\u{3045}\u{3047}\u{3049}\u{3063}\u{3083}\u{3085}\u{3087}\u{308E}\u{3095}\u{3096}\u{31F0}\u{31F1}\u{31F2}\u{31F3}\u{31F4}\u{31F5}\u{31F6}\u{31F7}\u{31F8}\u{31F9}\u{31FA}\u{31FB}\u{31FC}\u{31FD}\u{31FE}\u{31FF}\u{3005}\u{303B}\u{FF67}\u{FF68}\u{FF69}\u{FF6A}\u{FF6B}\u{FF6C}\u{FF6D}\u{FF6E}\u{FF6F}\u{FF70}\u{201D}\u{3009}\u{300B}\u{300D}\u{300F}\u{3011}\u{3015}\u{FF09}\u{FF3D}\u{FF5D}\u{FF63}";

/// `WrappingIndent`: how far the continuation lines of a wrapped line are indented.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WrappingIndent {
    None,
    Same,
    Indent,
    DeepIndent,
}

impl WrappingIndent {
    /// `editor.wrappingIndent`, `same` by default.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "none" => WrappingIndent::None,
            "indent" => WrappingIndent::Indent,
            "deepIndent" => WrappingIndent::DeepIndent,
            _ => WrappingIndent::Same,
        }
    }
}

/// What the line breaks depend on (`setWrappingSettings` and the factory's break characters).
#[derive(Clone, Debug, PartialEq)]
pub struct WrapOptions {
    /// `wrappingInfo.wrappingColumn`, at least 1.
    pub column: usize,
    /// `isViewportWrapping`: `on` or `bounded`, where the content width follows the viewport.
    pub viewport_wrapping: bool,
    pub tab_size: usize,
    pub indent: WrappingIndent,
    /// `editor.wordBreak` `keepAll`.
    pub keep_all: bool,
    pub wrap_on_escaped_line_feeds: bool,
    /// `typicalFullwidthCharacterWidth / typicalHalfwidthCharacterWidth` of the font.
    pub columns_for_full_width_char: f64,
    /// `effectiveFullwidthCharacterWidth` `twoCells`.
    pub two_cell_full_width: bool,
    pub break_before: String,
    pub break_after: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CharClass {
    None,
    BreakBefore,
    BreakAfter,
    BreakIdeographic,
}

/// `WrappingCharacterClassifier`.
#[derive(Debug)]
pub struct Classifier {
    classes: HashMap<char, CharClass>,
}

impl Classifier {
    pub fn new(break_before: &str, break_after: &str) -> Self {
        let mut classes = HashMap::new();
        for c in break_before.chars() {
            classes.insert(c, CharClass::BreakBefore);
        }
        for c in break_after.chars() {
            classes.insert(c, CharClass::BreakAfter);
        }
        Classifier { classes }
    }

    fn get(&self, c: char) -> CharClass {
        let code = c as u32;
        // Hiragana and Katakana, CJK Unified Ideographs Extension A and CJK Unified Ideographs.
        if matches!(code, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF) {
            return CharClass::BreakIdeographic;
        }
        self.classes.get(&c).copied().unwrap_or(CharClass::None)
    }
}

/// `ModelLineProjectionData` without injected text: the char offsets where each view line ends (the last one is
/// the line's length), their visible columns, and the indent of every view line after the first.
#[derive(Clone, Debug, PartialEq)]
pub struct LineBreaks {
    pub offsets: Vec<usize>,
    pub visible_columns: Vec<f64>,
    pub wrapped_indent: usize,
}

impl LineBreaks {
    /// `translateToOutputPosition` with `PositionAffinity.None`: an offset at a break starts the next view line.
    pub fn segment_of(&self, column: usize) -> usize {
        self.offsets.partition_point(|&offset| offset <= column).min(self.offsets.len() - 1)
    }

    pub fn segment_start(&self, segment: usize) -> usize {
        if segment == 0 { 0 } else { self.offsets[segment - 1] }
    }
}

/// `createLineBreaks` of monospaceLineBreaksComputer.ts. Upstream measures in pixels because injected text can
/// ask for any width; without injected text a char's pixel width is its column width times
/// `typicalHalfwidthCharacterWidth`, so this measures in columns. Offsets count chars, where upstream counts
/// UTF-16 units: a char past U+FFFF is one char here and two units there, and both never split it.
pub fn line_breaks(classifier: &Classifier, text: &str, options: &WrapOptions) -> Option<LineBreaks> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 1 {
        return None;
    }
    let tab_size = options.tab_size as f64;
    let full_width = if options.two_cell_full_width { 2. } else { options.columns_for_full_width_char };
    let char_width = |c: char, visible: f64| -> f64 {
        if c == '\t' {
            tab_size - visible % tab_size
        } else if (c as u32) > 0xFFFF {
            2.
        } else if is_full_width_character(c as u32) {
            full_width
        } else if (c as u32) < 32 {
            options.columns_for_full_width_char
        } else {
            1.
        }
    };
    let class_of = |c: char| if (c as u32) > 0xFFFF { CharClass::None } else { classifier.get(c) };
    let first_break_column = options.column as f64;
    let wrapped_indent = wrapped_indent_length(&chars, options.tab_size, options.column, full_width, options.indent);
    let wrapped_line_break_width = first_break_column - wrapped_indent as f64;

    let mut offsets = Vec::new();
    let mut visible_columns = Vec::new();
    let mut break_offset = 0;
    let mut break_visible = 0.;
    let mut breaking_width = first_break_column;
    let mut visible = char_width(chars[0], 0.);
    let mut prev_class = class_of(chars[0]);
    for (i, &c) in chars.iter().enumerate().skip(1) {
        let class = class_of(c);
        let width = char_width(c, visible);
        let mut escaped_line_feed = false;
        if options.wrap_on_escaped_line_feeds && is_escaped_line_break_at(&chars, i) {
            (break_offset, break_visible) = (i, visible);
            escaped_line_feed = true;
        } else if can_break(c, prev_class, class, options.keep_all) {
            (break_offset, break_visible) = (i, visible);
        }
        visible += width;
        if visible > breaking_width || escaped_line_feed {
            // Cannot break at `break_offset`: break at the current char.
            if break_offset == 0 || visible - break_visible > wrapped_line_break_width {
                (break_offset, break_visible) = (i, visible - width);
            }
            if break_offset > offsets.last().copied().unwrap_or(0) {
                offsets.push(break_offset);
                visible_columns.push(break_visible);
                breaking_width = break_visible + wrapped_line_break_width;
                break_offset = 0;
            }
        }
        prev_class = class;
    }
    if offsets.is_empty() {
        return None;
    }
    offsets.push(chars.len());
    visible_columns.push(visible);
    Some(LineBreaks { offsets, visible_columns, wrapped_indent })
}

/// `isEscapedLineBreakAtPosition`: `\n` written with an odd number of backslashes ends right before `i`.
fn is_escaped_line_break_at(chars: &[char], i: usize) -> bool {
    if i < 2 || chars[i - 1] != 'n' {
        return false;
    }
    let backslashes = chars[..i - 1].iter().rev().take_while(|&&c| c == '\\').count();
    backslashes < i - 1 && backslashes % 2 == 1
}

/// `canBreak`, Kinsoku Shori: no break after a leading character like an open bracket, nor before a trailing one
/// like a period.
fn can_break(c: char, prev: CharClass, class: CharClass, keep_all: bool) -> bool {
    c != ' '
        && ((prev == CharClass::BreakAfter && class != CharClass::BreakAfter)
            || (prev != CharClass::BreakBefore && class == CharClass::BreakBefore)
            || (!keep_all && prev == CharClass::BreakIdeographic && class != CharClass::BreakAfter)
            || (!keep_all && class == CharClass::BreakIdeographic && prev != CharClass::BreakBefore))
}

/// `computeWrappedTextIndentLength`: the leading whitespace's width plus the extra tabs, none when not even one
/// full-width char would fit after it.
fn wrapped_indent_length(chars: &[char], tab_size: usize, first_break_column: usize, full_width: f64, indent: WrappingIndent) -> usize {
    let extra_tabs = match indent {
        WrappingIndent::None => return 0,
        WrappingIndent::Same => 0,
        WrappingIndent::Indent => 1,
        WrappingIndent::DeepIndent => 2,
    };
    let Some(first_non_whitespace) = chars.iter().position(|&c| c != ' ' && c != '\t') else { return 0 };
    let mut length = 0;
    for &c in &chars[..first_non_whitespace] {
        length += if c == '\t' { tab_size - length % tab_size } else { 1 };
    }
    for _ in 0..extra_tabs {
        length += tab_size - length % tab_size;
    }
    if length as f64 + full_width > first_break_column as f64 { 0 } else { length }
}

/// textModel.ts `isDominatedByLongLines`: lines of 10000 UTF-16 units or more hold more of them than the others.
fn is_dominated_by_long_lines(lines: &[String]) -> bool {
    let (mut long, mut short) = (0, 0);
    for line in lines {
        let length = line.encode_utf16().count();
        if length >= 10_000 {
            long += length;
        } else {
            short += length;
        }
    }
    long > short
}

/// One editor's wrapping: the options of its last layout, the line breaks of every model line, and the first view
/// line of each model line for the folds it was computed with.
#[derive(Debug, Default)]
pub struct WordWrap {
    options: Option<WrapOptions>,
    classifier: Option<Classifier>,
    breaks: Vec<Option<LineBreaks>>,
    starts: RefCell<Option<(Vec<Range<usize>>, Rc<Vec<usize>>)>>,
    /// Toggle Word Wrap's state for the model (toggleWordWrap.ts `transientWordWrapState`, `wordWrapOverride2`).
    pub toggled: Option<bool>,
    /// The selections a move left with these cursor heads at the end of the view line before their wrap point
    /// (the cursor's view position, which a model position alone puts at the next view line's start).
    row_ends: Option<(Vec<Selection>, Vec<Position>)>,
    /// textModel.ts `isDominatedByLongLines` when the editor got the model (codeEditorWidget.ts `_attachModel`).
    pub dominated_by_long_lines: bool,
}

/// One view line: model line `line` from char `start` to `end`, drawn after `indent` columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub indent: usize,
    pub first: bool,
    pub last: bool,
}

impl WordWrap {
    pub fn new(lines: &[String], too_large_for_tokenization: bool) -> Self {
        WordWrap { dominated_by_long_lines: !too_large_for_tokenization && is_dominated_by_long_lines(lines), ..Default::default() }
    }

    pub fn options(&self) -> Option<&WrapOptions> {
        self.options.as_ref()
    }

    /// `setWrappingSettings`: breaks every line again when the options changed. Returns whether they did.
    pub fn configure(&mut self, options: Option<WrapOptions>, lines: &[String]) -> bool {
        if self.options == options && options.as_ref().is_none_or(|_| self.breaks.len() == lines.len()) {
            return false;
        }
        self.breaks.clear();
        if let Some(options) = &options {
            let classifier = match self.classifier.take() {
                Some(classifier) if self.options.as_ref().is_some_and(|old| (&old.break_before, &old.break_after) == (&options.break_before, &options.break_after)) => classifier,
                _ => Classifier::new(&options.break_before, &options.break_after),
            };
            self.breaks = lines.iter().map(|line| line_breaks(&classifier, line, options)).collect();
            self.classifier = Some(classifier);
        }
        self.options = options;
        self.starts.take();
        self.row_ends = None;
        true
    }

    /// `onModelLinesDeleted`/`onModelLinesInserted`/`onModelLineChanged`: lines `start..=end` became `inserted` lines,
    /// already in `lines`.
    pub fn lines_replaced(&mut self, lines: &[String], start: usize, end: usize, inserted: usize) {
        let (Some(options), Some(classifier)) = (&self.options, &self.classifier) else { return };
        let new: Vec<_> = lines[start..start + inserted].iter().map(|line| line_breaks(classifier, line, options)).collect();
        self.breaks.splice(start..=end, new);
        self.starts.take();
    }

    fn view_line_count_of(&self, line: usize) -> usize {
        self.breaks[line].as_ref().map_or(1, |breaks| breaks.offsets.len())
    }
}

impl OpenEditor {
    /// Whether the view wraps lines (`wrappingInfo.wrappingColumn` is not -1).
    pub fn wraps(&self) -> bool {
        self.word_wrap.options.is_some()
    }

    /// The first view line of every model line and the view line count last; a hidden line has none.
    fn view_starts(&self) -> Rc<Vec<usize>> {
        let hidden = self.folding.hidden();
        if let Some((cached_hidden, starts)) = &*self.word_wrap.starts.borrow()
            && cached_hidden == hidden
        {
            return starts.clone();
        }
        let mut starts = Vec::with_capacity(self.document.lines.len() + 1);
        let mut total = 0;
        for line in 0..self.document.lines.len() {
            starts.push(total);
            if folding::hidden_range(hidden, line).is_none() {
                total += self.word_wrap.view_line_count_of(line);
            }
        }
        starts.push(total);
        let starts = Rc::new(starts);
        *self.word_wrap.starts.borrow_mut() = Some((hidden.to_vec(), starts.clone()));
        starts
    }

    pub(super) fn wrapped_view_line_count(&self) -> usize {
        *self.view_starts().last().expect("the starts end with the count")
    }

    /// The first view line of model line `line`; a hidden line shows on its fold's line.
    pub(super) fn wrapped_view_line(&self, line: usize) -> usize {
        let shown = self.folding.model_line(self.folding.view_line(line));
        self.view_starts()[shown]
    }

    pub(super) fn wrapped_model_line(&self, view_line: usize) -> usize {
        let starts = self.view_starts();
        let line_count = self.document.lines.len();
        let view_line = view_line.min(starts[line_count].saturating_sub(1));
        starts[..line_count].partition_point(|&start| start <= view_line) - 1
    }

    /// The last view line of model line `line`, the first when it does not wrap or is hidden.
    pub fn last_view_line(&self, line: usize) -> usize {
        let view_line = self.view_line(line);
        match self.wraps() && !self.folding.is_hidden(line) {
            true => view_line + self.word_wrap.view_line_count_of(line) - 1,
            false => view_line,
        }
    }

    /// The model line and chars that view line `view_line` shows.
    pub fn segment(&self, view_line: usize) -> Segment {
        let line = self.model_line(view_line);
        let whole = Segment { line, start: 0, end: self.document.lines[line].chars().count(), indent: 0, first: true, last: true };
        if !self.wraps() {
            return whole;
        }
        let Some(breaks) = &self.word_wrap.breaks[line] else { return whole };
        let index = view_line - self.view_line(line);
        let first = index == 0;
        Segment { line, start: breaks.segment_start(index), end: breaks.offsets[index], indent: if first { 0 } else { breaks.wrapped_indent }, first, last: index + 1 == breaks.offsets.len() }
    }

    /// The view line `position` shows on; `row_end` keeps a position at a wrap point on the view line it ends.
    pub fn position_view_line(&self, position: Position, row_end: bool) -> usize {
        let view_line = self.view_line(position.line);
        if !self.wraps() || self.folding.is_hidden(position.line) {
            return view_line;
        }
        let Some(breaks) = &self.word_wrap.breaks[position.line] else { return view_line };
        let segment = breaks.segment_of(position.column);
        let at_break = segment > 0 && breaks.segment_start(segment) == position.column;
        view_line + segment - usize::from(row_end && at_break)
    }

    /// Whether cursor `index`'s head shows at the end of the view line before its wrap point.
    pub fn head_at_row_end(&self, index: usize) -> bool {
        let Some((selections, heads)) = &self.word_wrap.row_ends else { return false };
        *selections == self.selections && heads.contains(&self.selections[index].head)
    }

    /// The view line cursor `index`'s head shows on.
    pub fn head_view_line(&self, index: usize) -> usize {
        self.position_view_line(self.selections[index].head, self.head_at_row_end(index))
    }

    /// The x of column `column` of model line `line` within view line `segment`, in columns from the text's left.
    pub fn segment_visible_column(&self, segment: Segment, column: usize) -> usize {
        let text = &self.document.lines[segment.line];
        let tab_size = self.document.indentation.tab_size;
        segment.indent + visible_column(text, column, tab_size) - visible_column(text, segment.start, tab_size)
    }

    /// The column at visible column `visible` of view line `view_line`, kept inside it, and whether it is the
    /// view line's end before a wrap point.
    pub fn segment_column(&self, view_line: usize, visible: usize) -> (Position, bool) {
        let segment = self.segment(view_line);
        let text = &self.document.lines[segment.line];
        let tab_size = self.document.indentation.tab_size;
        let model_visible = visible.saturating_sub(segment.indent) + visible_column(text, segment.start, tab_size);
        let column = cursor::column_from_visible(text, model_visible, tab_size).clamp(segment.start, segment.end);
        (Position::new(segment.line, column), !segment.last && column == segment.end)
    }

    /// `MoveOperations.vertical` over view lines: keeps the visible column of the view line, plus the leftover,
    /// and goes to the document's edge when there is no view line to go to.
    fn vertical_in_view(&self, position: Position, row_end: bool, leftover: isize, down: bool) -> (Position, isize, bool) {
        let view_line = self.position_view_line(position, row_end);
        let segment = self.segment(view_line);
        let visible = self.segment_visible_column(segment, position.column) as isize + leftover;
        let last = self.last_shown_position();
        let was_at_edge = if down { position == last } else { position == Position::default() };
        let (moved, moved_end) = match (down, view_line) {
            (true, line) if line + 1 >= self.view_line_count() => (last, false),
            (false, 0) => (Position::default(), false),
            _ => self.segment_column(if down { view_line + 1 } else { view_line - 1 }, visible.max(0) as usize),
        };
        let moved_view_line = self.position_view_line(moved, moved_end);
        let moved_visible = self.segment_visible_column(self.segment(moved_view_line), moved.column) as isize;
        (moved, if was_at_edge { 0 } else { visible - moved_visible }, moved_end)
    }

    /// `cursorUp`/`cursorDown` with wrapped lines.
    pub(super) fn move_vertically_in_view(&mut self, down: bool, select: bool) {
        let mut selections = Vec::new();
        let mut leftovers = Vec::new();
        let mut row_ends = Vec::new();
        for (index, &selection) in self.selections.iter().enumerate() {
            let (anchor_leftover, head_leftover) = self.leftover(index);
            let (from, from_end) = match (select || selection.is_empty(), down) {
                (true, _) => (selection.head, self.head_at_row_end(index)),
                (false, false) => (selection.start(), false),
                (false, true) => (selection.end(), false),
            };
            let (head, leftover, row_end) = self.vertical_in_view(from, from_end, head_leftover, down);
            if row_end {
                row_ends.push(head);
            }
            selections.push(selection.moved(head, select));
            leftovers.push((if select { anchor_leftover } else { leftover }, leftover));
        }
        self.set_cursor_states(selections, leftovers);
        self.set_row_ends(row_ends);
    }

    /// `cursorHome`/`cursorEnd` with wrapped lines (`CursorMoveCommands._moveToLineStart`/`_moveToLineEnd`): first
    /// to the view line's start or end, then, already there or on the model line's first or last view line, the
    /// model line's.
    pub(super) fn move_to_line_edge_in_view(&mut self, end: bool, select: bool) {
        let mut selections = Vec::new();
        let mut row_ends = Vec::new();
        for index in 0..self.selections.len() {
            let selection = self.selections[index];
            let head = selection.head;
            let segment = self.segment(self.head_view_line(index));
            let text = &self.document.lines[head.line];
            let moved = if end {
                let at_row_end = self.head_at_row_end(index) || (segment.last && head.column == segment.end);
                match at_row_end || segment.last {
                    true => cursor::line_end(&self.document.lines, head),
                    false => {
                        row_ends.push(Position::new(head.line, segment.end));
                        Position::new(head.line, segment.end)
                    }
                }
            } else {
                let first_non_blank = text.chars().skip(segment.start).take(segment.end - segment.start).position(|c| c != ' ' && c != '\t').map_or(segment.end, |offset| segment.start + offset);
                match segment.first || head.column == first_non_blank {
                    true => cursor::line_start(&self.document.lines, head),
                    false => Position::new(head.line, segment.start),
                }
            };
            selections.push(selection.moved(moved, select));
        }
        self.set_cursor_states(selections, Vec::new());
        self.set_row_ends(row_ends);
    }

    /// Keeps `heads` at the end of their view lines while the selections stay as they are now.
    pub fn set_row_ends(&mut self, heads: Vec<Position>) {
        self.word_wrap.row_ends = (!heads.is_empty()).then(|| (self.selections.clone(), heads));
    }

    /// `getViewLineContent`: the wrapped indent's spaces, then the segment's text.
    pub fn view_line_text(&self, view_line: usize) -> Cow<'_, str> {
        let segment = self.segment(view_line);
        let text = &self.document.lines[segment.line];
        if segment.first && segment.last {
            return Cow::Borrowed(text);
        }
        Cow::Owned(" ".repeat(segment.indent) + &text.chars().skip(segment.start).take(segment.end - segment.start).collect::<String>())
    }

    /// `view_line_text` with its minimap token runs (byte ends and colors), cut from the model line's `runs`.
    pub fn view_line_minimap_data(&self, view_line: usize, runs: Vec<(usize, [u8; 3])>) -> (Cow<'_, str>, Vec<(usize, [u8; 3])>) {
        let segment = self.segment(view_line);
        let text = self.view_line_text(view_line);
        if segment.first && segment.last {
            return (text, runs);
        }
        let line = &self.document.lines[segment.line];
        let byte = |column: usize| line.char_indices().nth(column).map_or(line.len(), |(byte, _)| byte);
        let (start, end) = (byte(segment.start), byte(segment.end));
        let mut cut = Vec::new();
        if segment.indent > 0 && let Some(&(_, color)) = runs.first() {
            cut.push((segment.indent, color));
        }
        for (run_end, color) in runs.into_iter().filter(|(run_end, _)| *run_end > start) {
            cut.push((run_end.min(end) - start + segment.indent, color));
            if run_end >= end {
                break;
            }
        }
        (text, cut)
    }

    /// `convertModelPositionToViewPosition`: the view line and its column (the wrapped indent counted as chars).
    pub fn view_column(&self, position: Position) -> (usize, usize) {
        let view_line = self.position_view_line(position, false);
        let segment = self.segment(view_line);
        (view_line, segment.indent + position.column.clamp(segment.start, segment.end) - segment.start)
    }

    /// The line breaks' widest view line in columns, the indent included, or `None` without wrapping.
    pub fn wrapped_line_width(&self, view_line: usize) -> Option<usize> {
        if !self.wraps() {
            return None;
        }
        let segment = self.segment(view_line);
        Some(self.segment_visible_column(segment, segment.end))
    }
}

#[cfg(test)]
mod tests;
