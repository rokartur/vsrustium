//! The name of an untitled model (untitledTextEditorModel.ts `name` and `updateNameFromFirstLine`): the first
//! words of its first line, else the resource's name.

use crate::editor::OpenEditor;
use crate::editor::in_place_replace::DEFAULT_WORD_REGEXP;

const FIRST_LINE_NAME_MAX_LENGTH: usize = 40;
const FIRST_LINE_NAME_CANDIDATE_MAX_LENGTH: usize = FIRST_LINE_NAME_MAX_LENGTH * 10;

/// `name` of an untitled model, with `workbench.editor.untitled.labelFormat` `content` when `from_content`.
pub fn name(editor: &OpenEditor, from_content: bool) -> String {
    let first_line_words = (from_content && !editor.associated_file).then(|| first_line_words(&editor.document.lines[0])).flatten();
    first_line_words.unwrap_or_else(|| editor.document.name())
}

/// `updateNameFromFirstLine`: the first 400 UTF-16 units, trimmed, whitespace runs as one space, no U+202E,
/// cut at 40 units; `None` when that has no word.
pub fn first_line_words(line: &str) -> Option<String> {
    let candidate = take_utf16(line, FIRST_LINE_NAME_CANDIDATE_MAX_LENGTH);
    let mut normalized = String::new();
    for word in candidate.split(is_js_whitespace).filter(|word| !word.is_empty()) {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    let normalized = normalized.replace('\u{202E}', "");
    let name = take_utf16(&normalized, FIRST_LINE_NAME_MAX_LENGTH);
    DEFAULT_WORD_REGEXP.find(name).map(|_| name.to_string())
}

/// The longest prefix of whole chars within `units` UTF-16 code units.
fn take_utf16(text: &str, units: usize) -> &str {
    let mut length = 0;
    for (index, char) in text.char_indices() {
        length += char.len_utf16();
        if length > units {
            return &text[..index];
        }
    }
    text
}

/// JavaScript's `\s` and `trim()` set: Unicode white space and U+FEFF.
fn is_js_whitespace(char: char) -> bool {
    char.is_whitespace() || char == '\u{FEFF}'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_names_an_untitled_model() {
        // untitledTextEditor.test.ts 'model#onDidChangeName and input name'.
        let name = |line: &str| first_line_words(line);
        assert_eq!(name("foo").as_deref(), Some("foo"));
        assert_eq!(name(""), None);
        assert_eq!(name("        "), None);
        assert_eq!(name("([]}"), None, "require actual words");
        assert_eq!(name("([]}hello   ").as_deref(), Some("([]}hello"));
        assert_eq!(name("12345678901234567890123456789012345678901234567890").as_deref(), Some("1234567890123456789012345678901234567890"));
        assert_eq!(name("123456789012345678901234567890123456789\u{1F31E}").as_deref(), Some("123456789012345678901234567890123456789"), "#111235");
        assert_eq!(name("hello\u{202E}world").as_deref(), Some("helloworld"), "#190133");
        assert_eq!(name("   \t  Hello   \t World  ").as_deref(), Some("Hello World"));
    }
}
