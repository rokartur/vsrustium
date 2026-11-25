//! editor/contrib/inPlaceReplace/browser: Replace with Previous/Next Value (inPlaceReplace.ts,
//! inPlaceReplaceCommand.ts) over the editor worker's `navigateValueSet` and `BasicInplaceReplace`.

use std::ops::Range;
use std::sync::LazyLock;

use super::edit::{Command, command};
use super::smart_select::pattern_word_at;
use super::{ContentChanged, OpenEditor, Position, Replace, Selection, text_between};
use crate::languages::Languages;

/// wordHelper.ts `DEFAULT_WORD_REGEXP`: numbers with a fraction, else runs of chars that are not separators.
pub(crate) static DEFAULT_WORD_REGEXP: LazyLock<onig::Regex> =
    LazyLock::new(|| onig::Regex::new(r#"(-?\d*\.\d\w*)|([^\`\~\!\@\#\$\%\^\&\*\(\)\-\=\+\[\{\]\}\\\|\;\:\'\"\,\.\<\>\/\?\s]+)"#).expect("valid pattern"));

impl OpenEditor {
    /// `InPlaceReplaceController.run`: the selection's text, else the word at its start, steps to the next
    /// (`up`) or previous value. Returns the change and the inserted text's range, which the view outlines.
    pub fn in_place_replace(&mut self, up: bool, languages: &Languages) -> Option<(ContentChanged, (usize, Range<usize>))> {
        let mut selection = self.selection();
        if selection.start().line != selection.end().line {
            return None;
        }
        let word_pattern = languages.rules(&self.document.language_id).and_then(|rules| rules.word_pattern.as_ref());
        let (start, end, value) = navigate_value_set(&self.document.lines, selection, up, word_pattern.unwrap_or(&DEFAULT_WORD_REGEXP))?;
        let value_len = value.chars().count();
        let diff = value_len as isize - (selection.end().column - selection.start().column) as isize;
        if diff > 1 {
            let extended = Position::new(selection.end().line, selection.end().column + diff as usize - 1);
            selection = Selection { anchor: selection.start(), head: extended };
        }
        let highlight = (start.line, start.column..start.column + value_len);
        let cursor_state = Box::new(move |_: Option<Selection>, ranges: &[(Position, Position)]| {
            let inserted_end = ranges[0].1;
            if !selection.is_empty() {
                return Selection { anchor: Position::new(inserted_end.line, inserted_end.column.saturating_sub(value_len)), head: inserted_end };
            }
            Selection::caret(Position::new(inserted_end.line, selection.head.column.min(inserted_end.column)))
        });
        let commands = vec![Command { edits: vec![Replace::new(start, end, value)], tracked: None, cursor_state }];
        let id = if up { "editor.action.inPlaceReplace.down" } else { "editor.action.inPlaceReplace.up" };
        self.push_undo_stop();
        let change = self.execute_commands(commands, command(id));
        self.push_undo_stop();
        Some((change?, highlight))
    }
}

/// `EditorWorker.$navigateValueSet`: an empty selection takes the char after it; without a word at the
/// selection start there is no result.
fn navigate_value_set(lines: &[String], selection: Selection, up: bool, word_pattern: &onig::Regex) -> Option<(Position, Position, String)> {
    let (start, mut end) = (selection.start(), selection.end());
    let line = &lines[start.line];
    if start == end {
        end = Position::new(end.line, (end.column + 1).min(line.chars().count()));
    }
    let word = pattern_word_at(word_pattern, line, start.column)?;
    if let Some(value) = Some(text_between(lines, start, end)).filter(|text| !text.is_empty()).and_then(|text| next_value(&text, up)) {
        return Some((start, end, value));
    }
    let word_text: String = line.chars().skip(word.start).take(word.len()).collect();
    let value = Some(word_text).filter(|text| !text.is_empty()).and_then(|text| next_value(&text, up))?;
    Some((Position::new(start.line, word.start), Position::new(start.line, word.end), value))
}

/// `BasicInplaceReplace.doNavigateValueSet`.
fn next_value(text: &str, up: bool) -> Option<String> {
    number_replace(text, up).or_else(|| text_replace(text, up))
}

/// `numberReplace`: one up or down, through the text's decimal count (`1.5` gives `2.5`).
fn number_replace(value: &str, up: bool) -> Option<String> {
    let number = js_number(value)?;
    if number == 0. && !up {
        return None;
    }
    let decimals = value.len() as f64 - value.rfind('.').map_or(0., |dot| dot as f64 + 1.);
    let precision = 10f64.powf(decimals);
    let stepped = (number * precision).floor() + if up { precision } else { -precision };
    Some(js_number_to_string(stepped / precision))
}

/// `Number(value)` where it equals `parseFloat(value)`: a decimal literal or `Infinity`, else none.
fn js_number(value: &str) -> Option<f64> {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    if unsigned == "Infinity" {
        return Some(if value.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    if !value.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) {
        return None;
    }
    value.parse().ok()
}

/// `String(number)`: exponent notation from 1e21 and below 1e-6, as JavaScript formats numbers.
fn js_number_to_string(number: f64) -> String {
    if number.is_infinite() {
        return if number < 0. { "-Infinity".into() } else { "Infinity".into() };
    }
    if number == 0. {
        return "0".into();
    }
    if number.abs() >= 1e21 || number.abs() < 1e-6 {
        let formatted = format!("{number:e}");
        return match formatted.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
            _ => formatted,
        };
    }
    number.to_string()
}

const VALUE_SETS: [&[&str]; 4] = [
    &["true", "false"],
    &["True", "False"],
    &["Private", "Public", "Friend", "ReadOnly", "Partial", "Protected", "WriteOnly"],
    &["public", "protected", "private"],
];

/// `valueSetsReplace`: the next value of the first set holding the text, wrapping around.
fn text_replace(value: &str, up: bool) -> Option<String> {
    for set in VALUE_SETS {
        if let Some(index) = set.iter().position(|item| *item == value) {
            let next = if up { (index + 1) % set.len() } else { (index + set.len() - 1) % set.len() };
            return Some(set[next].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::edit::tests::editor;

    #[test]
    fn numbers_step_their_last_digit_and_value_sets_wrap() {
        assert_eq!(next_value("1", true).as_deref(), Some("2"));
        assert_eq!(next_value("1", false).as_deref(), Some("0"));
        assert_eq!(next_value("0", false), None);
        assert_eq!(next_value("1.5", true).as_deref(), Some("2.5"));
        assert_eq!(next_value("0.1", false).as_deref(), Some("-0.9"));
        assert_eq!(next_value("-1.25", true).as_deref(), Some("-0.25"));
        assert_eq!(next_value("9", true).as_deref(), Some("10"));
        assert_eq!(next_value("0x10", true), None);
        assert_eq!(next_value("true", true).as_deref(), Some("false"));
        assert_eq!(next_value("false", true).as_deref(), Some("true"));
        assert_eq!(next_value("Private", false).as_deref(), Some("WriteOnly"));
        assert_eq!(next_value("protected", true).as_deref(), Some("private"));
        assert_eq!(next_value("maybe", true), None);
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1.5e-7), "1.5e-7");
    }

    fn replace(text: &str, anchor: usize, head: usize, up: bool) -> (String, Selection) {
        let mut model = editor(text, "\n");
        model.set_selections(vec![Selection { anchor: Position::new(0, anchor), head: Position::new(0, head) }]);
        model.in_place_replace(up, &Languages::new(&[]));
        (model.document.lines[0].clone(), model.selections[0])
    }

    #[test]
    fn a_caret_replaces_its_word_and_a_selection_its_text() {
        let range = |anchor, head| Selection { anchor: Position::new(0, anchor), head: Position::new(0, head) };
        // An empty selection grows by the value's length less one, so the new word ends selected.
        assert_eq!(replace("x = true;", 5, 5, true), ("x = false;".into(), range(4, 9)));
        // A caret before a number steps the one char after it, as the worker reads it first.
        assert_eq!(replace("x = 41;", 4, 4, true), ("x = 51;".into(), range(4, 4)));
        assert_eq!(replace("x = 41;", 4, 6, true), ("x = 42;".into(), range(4, 6)));
        assert_eq!(replace("x = 1.5;", 4, 7, false), ("x = 0.5;".into(), range(4, 7)));
        assert_eq!(replace("x = y;", 4, 4, true), ("x = y;".into(), range(4, 4)));
    }
}
