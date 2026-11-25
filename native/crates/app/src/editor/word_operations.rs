//! cursorWordOperations.ts `WordOperations` and `WordPartOperations`: word moves, word deletes and Delete Word,
//! over `WordCharacterClassifier` (`editor.wordSeparators`, and `Intl.Segmenter` words with `editor.wordSegmenterLocales`).
//! Columns are chars (cursor.rs); upstream's are 1-based UTF-16 columns.

use icu_segmenter::WordSegmenter;
use icu_segmenter::options::WordBreakInvariantOptions;

use super::{Position, Selection};

/// `USUAL_WORD_SEPARATORS`, the default of `editor.wordSeparators`.
pub const DEFAULT_WORD_SEPARATORS: &str = super::WORD_SEPARATORS;

#[derive(Clone, Copy, Debug, PartialEq)]
enum CharClass {
    Regular,
    Whitespace,
    Separator,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum WordType {
    Regular,
    Separator,
}

/// `IFindWordResult`: `next` is the class of the char that ended the word.
#[derive(Clone, Copy, Debug)]
struct Word {
    start: usize,
    end: usize,
    kind: WordType,
    next: CharClass,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WordNavigation {
    WordStart,
    WordStartFast,
    WordEnd,
    WordAccessibility,
}

/// `WordCharacterClassifier`: space and tab are whitespace, the separators separate, and with segmenter
/// locales `Intl.Segmenter` word-like segments are words of their own.
pub struct WordClassifier {
    separators: Vec<char>,
    segment: bool,
}

impl WordClassifier {
    /// `getMapForWordSeparators` with the locales `WordSegmenterLocales.validate` kept.
    pub fn new(separators: &str, segmenter_locales: &[String]) -> Self {
        WordClassifier { separators: separators.chars().collect(), segment: !segmenter_locales.is_empty() }
    }

    fn get(&self, c: char) -> CharClass {
        match c {
            ' ' | '\t' => CharClass::Whitespace,
            c if self.separators.contains(&c) => CharClass::Separator,
            _ => CharClass::Regular,
        }
    }

    /// The word-like segments of `Intl.Segmenter(locales, { granularity: 'word' })`, as char ranges.
    fn intl_words(&self, line: &[char]) -> Vec<(usize, usize)> {
        if !self.segment {
            return Vec::new();
        }
        let text: String = line.iter().collect();
        let char_index = |byte: usize| text[..byte].chars().count();
        // ICU4C, which V8's Intl.Segmenter runs, breaks Chinese and Japanese by dictionary.
        let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
        let mut breaks = segmenter.segment_str(&text);
        let mut words = Vec::new();
        let mut start = breaks.next().unwrap_or(0);
        while let Some(end) = breaks.next() {
            if breaks.is_word_like() {
                words.push((char_index(start), char_index(end)));
            }
            start = end;
        }
        words
    }

    /// `findPrevIntlWordBeforeOrAtOffset`.
    fn previous_intl_word(&self, line: &[char], offset: Option<usize>) -> Option<(usize, usize)> {
        let offset = offset?;
        self.intl_words(line).into_iter().take_while(|&(start, _)| start <= offset).last()
    }

    /// `findNextIntlWordAtOrAfterOffset`.
    fn next_intl_word(&self, line: &[char], offset: usize) -> Option<(usize, usize)> {
        self.intl_words(line).into_iter().find(|&(start, _)| start >= offset)
    }
}

/// `WordSegmenterLocales.validate`: a string or an array of strings, keeping the locales the segmenter knows.
/// shortcut: any well-formed BCP 47 tag counts as supported, where V8 also drops tags ICU has no data for.
pub fn segmenter_locales(value: Option<&serde_json::Value>) -> Vec<String> {
    let values = match value {
        Some(serde_json::Value::String(locale)) => vec![locale.as_str()],
        Some(serde_json::Value::Array(locales)) => locales.iter().filter_map(|locale| locale.as_str()).collect(),
        _ => Vec::new(),
    };
    values.into_iter().filter(|locale| icu_locale_core::Locale::try_from_str(locale).is_ok()).map(str::to_string).collect()
}

fn chars(lines: &[String], line: usize) -> Vec<char> {
    lines[line].chars().collect()
}

fn is_whitespace(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// `_doFindPreviousWordOnLine`: the word that ends at or before `column`.
fn previous_word(line: &[char], classifier: &WordClassifier, column: usize) -> Option<Word> {
    let mut kind = None;
    let previous_intl = classifier.previous_intl_word(line, column.checked_sub(1));
    for index in (0..column).rev() {
        let class = classifier.get(line[index]);
        if let Some((start, end)) = previous_intl.filter(|&(start, _)| start == index) {
            return Some(Word { start, end, kind: WordType::Regular, next: class });
        }
        let word = |kind| Some(Word { start: index + 1, end: end_of_word(line, classifier, kind, index + 1), kind, next: class });
        match class {
            CharClass::Regular if kind == Some(WordType::Separator) => return word(WordType::Separator),
            CharClass::Regular => kind = Some(WordType::Regular),
            CharClass::Separator if kind == Some(WordType::Regular) => return word(WordType::Regular),
            CharClass::Separator => kind = Some(WordType::Separator),
            CharClass::Whitespace => {
                if let Some(kind) = kind {
                    return word(kind);
                }
            }
        }
    }
    kind.map(|kind| Word { start: 0, end: end_of_word(line, classifier, kind, 0), kind, next: CharClass::Whitespace })
}

/// `_findEndOfWord`.
fn end_of_word(line: &[char], classifier: &WordClassifier, kind: WordType, start: usize) -> usize {
    let next_intl = classifier.next_intl_word(line, start);
    for (index, &c) in line.iter().enumerate().skip(start) {
        if next_intl.is_some_and(|(_, end)| index == end) {
            return index;
        }
        match (classifier.get(c), kind) {
            (CharClass::Whitespace, _) | (CharClass::Separator, WordType::Regular) | (CharClass::Regular, WordType::Separator) => return index,
            _ => {}
        }
    }
    line.len()
}

/// `_doFindNextWordOnLine`: the word that starts at or after `column`.
fn next_word(line: &[char], classifier: &WordClassifier, column: usize) -> Option<Word> {
    let mut kind = None;
    let next_intl = classifier.next_intl_word(line, column);
    for (index, &c) in line.iter().enumerate().skip(column) {
        let class = classifier.get(c);
        if let Some((start, end)) = next_intl.filter(|&(start, _)| start == index) {
            return Some(Word { start, end, kind: WordType::Regular, next: class });
        }
        let word = |kind| Some(Word { start: start_of_word(line, classifier, kind, index - 1), end: index, kind, next: class });
        match class {
            CharClass::Regular if kind == Some(WordType::Separator) => return word(WordType::Separator),
            CharClass::Regular => kind = Some(WordType::Regular),
            CharClass::Separator if kind == Some(WordType::Regular) => return word(WordType::Regular),
            CharClass::Separator => kind = Some(WordType::Separator),
            CharClass::Whitespace => {
                if let Some(kind) = kind {
                    return word(kind);
                }
            }
        }
    }
    kind.map(|kind| Word { start: start_of_word(line, classifier, kind, line.len() - 1), end: line.len(), kind, next: CharClass::Whitespace })
}

/// `_findStartOfWord`, walking left from `start` (inclusive).
fn start_of_word(line: &[char], classifier: &WordClassifier, kind: WordType, start: usize) -> usize {
    let previous_intl = classifier.previous_intl_word(line, Some(start));
    for index in (0..=start).rev() {
        if previous_intl.is_some_and(|(intl_start, _)| index == intl_start) {
            return index;
        }
        match (classifier.get(line[index]), kind) {
            (CharClass::Whitespace, _) | (CharClass::Separator, WordType::Regular) | (CharClass::Regular, WordType::Separator) => return index + 1,
            _ => {}
        }
    }
    0
}

/// A word of one separator followed by a regular char, as "." in "a.b".
fn is_lone_separator(word: &Word) -> bool {
    word.kind == WordType::Separator && word.end - word.start == 1 && word.next == CharClass::Regular
}

/// `moveWordLeft`.
pub fn move_word_left(classifier: &WordClassifier, lines: &[String], position: Position, navigation: WordNavigation, has_multicursor: bool) -> Position {
    let Position { mut line, mut column } = position;
    if column == 0 && line > 0 {
        line -= 1;
        column = lines[line].chars().count();
    }
    let text = chars(lines, line);
    let mut word = previous_word(&text, classifier, column);
    match navigation {
        WordNavigation::WordStart => {}
        WordNavigation::WordStartFast => {
            // Multiple cursors skip nothing, so they all stop at the same kind of place.
            if let Some(found) = word.filter(|word| !has_multicursor && is_lone_separator(word)) {
                word = previous_word(&text, classifier, found.start);
            }
        }
        WordNavigation::WordAccessibility => {
            while let Some(found) = word.filter(|word| word.kind == WordType::Separator) {
                word = previous_word(&text, classifier, found.start);
            }
        }
        WordNavigation::WordEnd => {
            if let Some(found) = word.filter(|word| column <= word.end) {
                word = previous_word(&text, classifier, found.start);
            }
            return Position::new(line, word.map_or(0, |word| word.end));
        }
    }
    Position::new(line, word.map_or(0, |word| word.start))
}

/// The snake_case, kebab-case and camelCase boundary between `left` and `right`; `right_right` follows `right`.
fn is_word_part_boundary(left: char, right: char, right_right: Option<char>, rightward: bool) -> bool {
    let snake = if rightward { left != '_' && right == '_' } else { left == '_' && right != '_' };
    let kebab = if rightward { left != '-' && right == '-' } else { left == '-' && right != '-' };
    let camel = (left.is_ascii_lowercase() || left.is_ascii_digit()) && right.is_ascii_uppercase();
    let one_letter_words = left.is_ascii_uppercase() && right.is_ascii_uppercase() && right_right.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    snake || kebab || camel || one_letter_words
}

/// `_moveWordPartLeft`.
fn move_to_word_part_left(lines: &[String], position: Position) -> Position {
    let Position { line, column } = position;
    if column == 0 {
        return if line > 0 { Position::new(line - 1, lines[line - 1].chars().count()) } else { position };
    }
    let text = chars(lines, line);
    for index in (1..column).rev() {
        if is_word_part_boundary(text[index - 1], text[index], text.get(index + 1).copied(), false) {
            return Position::new(line, index);
        }
    }
    Position::new(line, 0)
}

/// `moveWordRight`.
pub fn move_word_right(classifier: &WordClassifier, lines: &[String], position: Position, navigation: WordNavigation) -> Position {
    let Position { mut line, mut column } = position;
    let moved_down = column == lines[line].chars().count() && line + 1 < lines.len();
    if moved_down {
        line += 1;
        column = 0;
    }
    let text = chars(lines, line);
    let mut word = next_word(&text, classifier, column);
    match navigation {
        WordNavigation::WordEnd => {
            if let Some(found) = word.filter(is_lone_separator) {
                word = next_word(&text, classifier, found.end);
            }
            return Position::new(line, word.map_or(text.len(), |word| word.end));
        }
        WordNavigation::WordAccessibility => {
            // After a move down the cursor counts as before the first char, so a word there is not skipped.
            while let Some(found) = word.filter(|word| word.kind == WordType::Separator || (!moved_down && word.start <= column)) {
                word = next_word(&text, classifier, found.end);
            }
        }
        WordNavigation::WordStart | WordNavigation::WordStartFast => {
            if let Some(found) = word.filter(|word| !moved_down && column >= word.start) {
                word = next_word(&text, classifier, found.end);
            }
        }
    }
    Position::new(line, word.map_or(text.len(), |word| word.start))
}

/// `_moveWordPartRight`.
fn move_to_word_part_right(lines: &[String], position: Position) -> Position {
    let Position { line, column } = position;
    let text = chars(lines, line);
    if column == text.len() {
        return if line + 1 < lines.len() { Position::new(line + 1, 0) } else { position };
    }
    for index in column + 1..text.len() {
        if is_word_part_boundary(text[index - 1], text[index], text.get(index + 1).copied(), true) {
            return Position::new(line, index);
        }
    }
    Position::new(line, text.len())
}

/// `WordPartOperations.moveWordPartLeft`: the last of the word start, word end and word part stops.
pub fn move_word_part_left(classifier: &WordClassifier, lines: &[String], position: Position, has_multicursor: bool) -> Position {
    let mut candidates = [
        move_word_left(classifier, lines, position, WordNavigation::WordStart, has_multicursor),
        move_word_left(classifier, lines, position, WordNavigation::WordEnd, has_multicursor),
        move_to_word_part_left(lines, position),
    ];
    candidates.sort();
    candidates[2]
}

/// `WordPartOperations.moveWordPartRight`: the first of the word start, word end and word part stops.
pub fn move_word_part_right(classifier: &WordClassifier, lines: &[String], position: Position) -> Position {
    let mut candidates = [
        move_word_right(classifier, lines, position, WordNavigation::WordStart),
        move_word_right(classifier, lines, position, WordNavigation::WordEnd),
        move_to_word_part_right(lines, position),
    ];
    candidates.sort();
    candidates[0]
}

/// A word move command: its navigation, direction, whether it selects, and whether it uses the default word
/// separators instead of `editor.wordSeparators` (the accessibility moves).
#[derive(Clone, Copy)]
pub struct WordMove {
    pub navigation: WordNavigation,
    pub right: bool,
    pub select: bool,
    pub default_separators: bool,
}

/// The `MoveWordCommand`s of wordOperations.ts, by id.
pub const MOVE_COMMANDS: [(&str, WordMove); 16] = {
    use WordNavigation::*;
    const fn word_move(navigation: WordNavigation, right: bool, select: bool) -> WordMove {
        WordMove { navigation, right, select, default_separators: matches!(navigation, WordAccessibility) }
    }
    [
        ("cursorWordStartLeft", word_move(WordStart, false, false)),
        ("cursorWordEndLeft", word_move(WordEnd, false, false)),
        ("cursorWordLeft", word_move(WordStartFast, false, false)),
        ("cursorWordStartLeftSelect", word_move(WordStart, false, true)),
        ("cursorWordEndLeftSelect", word_move(WordEnd, false, true)),
        ("cursorWordLeftSelect", word_move(WordStartFast, false, true)),
        ("cursorWordStartRight", word_move(WordStart, true, false)),
        ("cursorWordEndRight", word_move(WordEnd, true, false)),
        ("cursorWordRight", word_move(WordEnd, true, false)),
        ("cursorWordStartRightSelect", word_move(WordStart, true, true)),
        ("cursorWordEndRightSelect", word_move(WordEnd, true, true)),
        ("cursorWordRightSelect", word_move(WordEnd, true, true)),
        ("cursorWordAccessibilityLeft", word_move(WordAccessibility, false, false)),
        ("cursorWordAccessibilityLeftSelect", word_move(WordAccessibility, false, true)),
        ("cursorWordAccessibilityRight", word_move(WordAccessibility, true, false)),
        ("cursorWordAccessibilityRightSelect", word_move(WordAccessibility, true, true)),
    ]
};

#[derive(Clone, Copy)]
pub enum WordDeleteKind {
    Left(WordNavigation),
    Right(WordNavigation),
    PartLeft,
    PartRight,
}

/// A `DeleteWordCommand`: what it deletes and whether blanks next to the cursor go alone.
#[derive(Clone, Copy)]
pub struct WordDelete {
    pub kind: WordDeleteKind,
    pub whitespace_heuristics: bool,
}

/// The `DeleteWordCommand`s of wordOperations.ts and wordPartOperations.ts, by id.
pub const DELETE_COMMANDS: [(&str, WordDelete); 8] = {
    use WordDeleteKind::*;
    use WordNavigation::*;
    const fn word_delete(kind: WordDeleteKind, whitespace_heuristics: bool) -> WordDelete {
        WordDelete { kind, whitespace_heuristics }
    }
    [
        ("deleteWordStartLeft", word_delete(Left(WordStart), false)),
        ("deleteWordEndLeft", word_delete(Left(WordEnd), false)),
        ("deleteWordLeft", word_delete(Left(WordStart), true)),
        ("deleteWordStartRight", word_delete(Right(WordStart), false)),
        ("deleteWordEndRight", word_delete(Right(WordEnd), false)),
        ("deleteWordRight", word_delete(Right(WordEnd), true)),
        ("deleteWordPartLeft", word_delete(PartLeft, true)),
        ("deleteWordPartRight", word_delete(PartRight, true)),
    ]
};

/// `editor.autoClosingDelete`, `editor.autoClosingBrackets` and `editor.autoClosingQuotes` with the language's
/// auto-closing pairs and the auto-closed characters of the cursors (start positions).
pub struct AutoClosing<'a> {
    pub delete: &'a str,
    pub brackets: &'a str,
    pub quotes: &'a str,
    pub pairs: &'a [(String, String)],
    pub auto_closed: &'a [Position],
}

/// `DeleteWordContext`.
pub struct DeleteWordContext<'a> {
    pub classifier: &'a WordClassifier,
    pub lines: &'a [String],
    pub selection: Selection,
    pub whitespace_heuristics: bool,
    pub auto_closing: &'a AutoClosing<'a>,
}

pub type Range = (Position, Position);

fn ordered(a: Position, b: Position) -> Range {
    if a <= b { (a, b) } else { (b, a) }
}

/// `DeleteOperations.isAutoClosingPairDelete` for one cursor: the cursor sits between an auto-closing pair's
/// one-char open and its close.
fn is_auto_closing_pair_delete(auto_closing: &AutoClosing, lines: &[String], selection: Selection) -> bool {
    if auto_closing.brackets == "never" && auto_closing.quotes == "never" || auto_closing.delete == "never" || !selection.is_empty() {
        return false;
    }
    let position = selection.head;
    let text = chars(lines, position.line);
    if position.column < 1 || position.column >= text.len() {
        return false;
    }
    let (character, after) = (text[position.column - 1], text[position.column]);
    let candidates: Vec<_> = auto_closing.pairs.iter().filter(|(open, _)| open.ends_with(character)).collect();
    if candidates.is_empty() {
        return false;
    }
    let setting = if matches!(character, '\'' | '"' | '`') { auto_closing.quotes } else { auto_closing.brackets };
    if setting == "never" {
        return false;
    }
    let found = candidates.iter().any(|(open, close)| open.chars().eq([character]) && close.chars().eq([after]));
    // `auto` deletes only a pair the editor inserted itself.
    found && (auto_closing.delete != "auto" || auto_closing.auto_closed.contains(&position))
}

/// `_deleteWordLeftWhitespace`: two or more blanks before the cursor.
fn delete_word_left_whitespace(lines: &[String], position: Position) -> Option<Range> {
    let text = chars(lines, position.line);
    let blanks = text[..position.column].iter().rev().take_while(|&&c| is_whitespace(c)).count();
    (blanks >= 2).then(|| (Position::new(position.line, position.column - blanks), position))
}

/// `deleteWordLeft`; `None` at the start of the document.
pub fn delete_word_left(ctx: &DeleteWordContext, navigation: WordNavigation) -> Option<Range> {
    let selection = ctx.selection;
    if !selection.is_empty() {
        return Some((selection.start(), selection.end()));
    }
    let position = selection.head;
    if is_auto_closing_pair_delete(ctx.auto_closing, ctx.lines, selection) {
        return Some((Position::new(position.line, position.column - 1), Position::new(position.line, position.column + 1)));
    }
    if position == Position::new(0, 0) {
        return None;
    }
    if ctx.whitespace_heuristics
        && let Some(range) = delete_word_left_whitespace(ctx.lines, position)
    {
        return Some(range);
    }
    let text = chars(ctx.lines, position.line);
    let mut word = previous_word(&text, ctx.classifier, position.column);
    if navigation != WordNavigation::WordStart
        && let Some(found) = word.filter(|word| position.column <= word.end)
    {
        word = previous_word(&text, ctx.classifier, found.start);
    }
    let start = match word {
        Some(word) if navigation == WordNavigation::WordStart => Position::new(position.line, word.start),
        Some(word) => Position::new(position.line, word.end),
        None if position.column > 0 => Position::new(position.line, 0),
        None => Position::new(position.line - 1, ctx.lines[position.line - 1].chars().count()),
    };
    Some((start, position))
}

/// `deleteInsideWord`: the whitespace around the cursor, else the word it touches with the whitespace after
/// it (or before it at the line end); `only_word` leaves the whitespace.
pub fn delete_inside_word(classifier: &WordClassifier, lines: &[String], selection: Selection, only_word: bool) -> Range {
    if !selection.is_empty() {
        return (selection.start(), selection.end());
    }
    let position = selection.head;
    delete_inside_word_whitespace(lines, position).unwrap_or_else(|| delete_inside_word_range(classifier, lines, position, only_word))
}

/// `_deleteInsideWordWhitespace`.
fn delete_inside_word_whitespace(lines: &[String], position: Position) -> Option<Range> {
    let text = chars(lines, position.line);
    if text.is_empty() {
        return None;
    }
    let mut left = position.column.saturating_sub(1);
    let mut right = position.column.min(text.len() - 1);
    if !is_whitespace(text[left]) || !is_whitespace(text[right]) {
        return None;
    }
    while left > 0 && is_whitespace(text[left - 1]) {
        left -= 1;
    }
    while right + 1 < text.len() && is_whitespace(text[right + 1]) {
        right += 1;
    }
    Some((Position::new(position.line, left), Position::new(position.line, right + 1)))
}

/// `_deleteInsideWordDetermineDeleteRange`.
fn delete_inside_word_range(classifier: &WordClassifier, lines: &[String], position: Position, only_word: bool) -> Range {
    let Position { line, column } = position;
    let text = chars(lines, line);
    if text.is_empty() {
        if line > 0 {
            return (Position::new(line - 1, lines[line - 1].chars().count()), Position::new(line, 0));
        }
        if line + 1 < lines.len() {
            return (Position::new(line, 0), Position::new(line + 1, 0));
        }
        return (position, position);
    }
    let touches = |word: &Word| word.start <= column && column <= word.end;
    let with_position = |start: usize, end: usize| (Position::new(line, start.min(column)), Position::new(line, end.max(column)));
    let word_and_whitespace = |word: Word| {
        let (mut start, mut end) = (word.start, word.end);
        if only_word {
            return with_position(start, end);
        }
        let expanded_right = end < text.len() && is_whitespace(text[end]);
        while end < text.len() && is_whitespace(text[end]) {
            end += 1;
        }
        while !expanded_right && start > 0 && is_whitespace(text[start - 1]) {
            start -= 1;
        }
        with_position(start, end)
    };
    let previous = previous_word(&text, classifier, column);
    if let Some(word) = previous.filter(touches) {
        return word_and_whitespace(word);
    }
    let next = next_word(&text, classifier, column);
    if let Some(word) = next.filter(touches) {
        return word_and_whitespace(word);
    }
    match (previous, next) {
        (Some(previous), Some(next)) => with_position(previous.end, next.start),
        (Some(word), None) | (None, Some(word)) => with_position(word.start, word.end),
        (None, None) => with_position(0, text.len()),
    }
}

/// `_deleteWordRightWhitespace`: the blanks after the cursor.
fn delete_word_right_whitespace(lines: &[String], position: Position) -> Option<Range> {
    let text = chars(lines, position.line);
    let blanks = text[position.column..].iter().take_while(|&&c| is_whitespace(c)).count();
    (blanks > 0).then(|| (position, Position::new(position.line, position.column + blanks)))
}

/// `deleteWordRight`; `None` at the end of the document.
pub fn delete_word_right(ctx: &DeleteWordContext, navigation: WordNavigation) -> Option<Range> {
    let selection = ctx.selection;
    if !selection.is_empty() {
        return Some((selection.start(), selection.end()));
    }
    let position = selection.head;
    let text = chars(ctx.lines, position.line);
    let last_line = position.line + 1 == ctx.lines.len();
    if last_line && position.column == text.len() {
        return None;
    }
    if ctx.whitespace_heuristics
        && let Some(range) = delete_word_right_whitespace(ctx.lines, position)
    {
        return Some(range);
    }
    let mut word = next_word(&text, ctx.classifier, position.column);
    if navigation != WordNavigation::WordEnd
        && let Some(found) = word.filter(|word| position.column >= word.start)
    {
        word = next_word(&text, ctx.classifier, found.end);
    }
    let end = match word {
        Some(word) if navigation == WordNavigation::WordEnd => Position::new(position.line, word.end),
        Some(word) => Position::new(position.line, word.start),
        None if position.column < text.len() || last_line => Position::new(position.line, text.len()),
        None => {
            let next_line = chars(ctx.lines, position.line + 1);
            Position::new(position.line + 1, next_word(&next_line, ctx.classifier, 0).map_or(next_line.len(), |word| word.start))
        }
    };
    Some((position, end))
}

/// `WordPartOperations.deleteWordPartLeft`: of the word start, word end and word part deletes, the one that
/// ends last; at the start of the document nothing.
pub fn delete_word_part_left(ctx: &DeleteWordContext) -> Option<Range> {
    let selection = ctx.selection;
    let word_part = match selection.is_empty() {
        true => ordered(selection.head, move_to_word_part_left(ctx.lines, selection.head)),
        false => (selection.start(), selection.end()),
    };
    let mut candidates: Vec<Range> = [delete_word_left(ctx, WordNavigation::WordStart), delete_word_left(ctx, WordNavigation::WordEnd), Some(word_part)].into_iter().flatten().collect();
    // `Range.compareRangesUsingEnds`.
    candidates.sort_by_key(|&(start, end)| (end, start));
    candidates.get(2).copied()
}

/// `WordPartOperations.deleteWordPartRight`: of the word start, word end and word part deletes, the one that
/// starts first.
pub fn delete_word_part_right(ctx: &DeleteWordContext) -> Range {
    let selection = ctx.selection;
    let word_part = match selection.is_empty() {
        true => ordered(selection.head, move_to_word_part_right(ctx.lines, selection.head)),
        false => (selection.start(), selection.end()),
    };
    let candidates = [delete_word_right(ctx, WordNavigation::WordStart), delete_word_right(ctx, WordNavigation::WordEnd), Some(word_part)];
    candidates.into_iter().flatten().min().expect("the word part delete is always a candidate")
}

#[cfg(test)]
mod tests;
