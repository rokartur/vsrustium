//! The indentation a text model starts with: the configured `editor.*` values and `guessIndentation`
//! (editor/common/model/indentationGuesser.ts). configurationEditing.ts indents new properties with it.
//! app/src/editor.rs holds a second `guess_indentation` port over its own line type; merge them when the text
//! model moves out of the app crate.

use std::path::Path;

use crate::configuration::{Configuration, Overrides};

/// The `editor.*` values of `getCreationOptions(language, resource)` that decide a model's indentation.
#[derive(PartialEq)]
pub struct CreationOptions {
    pub tab_size: usize,
    /// `editor.indentSize`; `None` for "tabSize", where it follows the tab size.
    pub indent_size: Option<usize>,
    pub insert_spaces: bool,
    pub detect_indentation: bool,
}

/// `getCreationOptions` for a file and its language, validated as EditorOptions does: `tabSize` clamped to
/// 1..=100, a wrong type falls back to the default.
pub fn creation_options(configuration: &Configuration, resource: &Path, language_id: &str) -> CreationOptions {
    let overrides = Overrides { resource: Some(resource), override_identifier: Some(language_id) };
    let editor = configuration.get_value(Some("editor"), &overrides).expect("editor.* settings are registered");
    CreationOptions {
        tab_size: editor["tabSize"].as_f64().map_or(4, |size| size.clamp(1., 100.) as usize),
        indent_size: editor["indentSize"].as_f64().map(|size| size.clamp(1., 100.) as usize),
        insert_spaces: editor["insertSpaces"].as_bool().unwrap_or(true),
        detect_indentation: editor["detectIndentation"].as_bool().unwrap_or(true),
    }
}

/// The indentation of a model of `lines`: guessed from the configured values when `editor.detectIndentation` is on.
pub fn model_indentation(lines: &[&str], options: &CreationOptions) -> (usize, bool) {
    match options.detect_indentation {
        true => guess_indentation(lines, options.tab_size, options.insert_spaces),
        false => (options.tab_size, options.insert_spaces),
    }
}
/// `guessIndentation(source, defaultTabSize, defaultInsertSpaces)` over UTF-16 code units as upstream counts them.
pub fn guess_indentation(lines: &[&str], default_tab_size: usize, default_insert_spaces: bool) -> (usize, bool) {
    const ALLOWED_TAB_SIZE_GUESSES: [usize; 7] = [2, 4, 6, 8, 3, 5, 7];
    const MAX_ALLOWED_TAB_SIZE_GUESS: usize = 8;
    let (mut lines_with_tabs, mut lines_with_spaces) = (0, 0);
    let mut previous_line: Vec<u16> = Vec::new();
    let mut previous_indentation = 0;
    let mut spaces_diff_count = [0usize; 9];
    for line in lines.iter().take(10000) {
        let line: Vec<u16> = line.encode_utf16().collect();
        let Some(indentation) = line.iter().position(|&c| c != u16::from(b' ') && c != u16::from(b'\t')) else { continue };
        let tabs = line[..indentation].iter().filter(|&&c| c == u16::from(b'\t')).count();
        if tabs > 0 {
            lines_with_tabs += 1;
        } else if indentation > 1 {
            lines_with_spaces += 1;
        }
        let (spaces, looks_like_alignment) = spaces_diff(&previous_line, previous_indentation, &line, indentation);
        if looks_like_alignment && !(default_insert_spaces && default_tab_size == spaces) {
            continue;
        }
        if spaces <= MAX_ALLOWED_TAB_SIZE_GUESS {
            spaces_diff_count[spaces] += 1;
        }
        previous_line = line;
        previous_indentation = indentation;
    }

    let mut insert_spaces = default_insert_spaces;
    if lines_with_tabs != lines_with_spaces {
        insert_spaces = lines_with_tabs < lines_with_spaces;
    }
    let mut tab_size = default_tab_size;
    if insert_spaces {
        let mut best_score = 0;
        for size in ALLOWED_TAB_SIZE_GUESSES {
            if spaces_diff_count[size] > best_score {
                best_score = spaces_diff_count[size];
                tab_size = size;
            }
        }
        // Indented 4 most often but 2 nearly as often: the file is probably indented by 2 with nested blocks.
        if tab_size == 4 && spaces_diff_count[4] > 0 && spaces_diff_count[2] > 0 && spaces_diff_count[2] * 3 >= spaces_diff_count[4] * 2 {
            tab_size = 2;
        }
    }
    (tab_size, insert_spaces)
}

/// `spacesDiff`: how much the indentation of `b` differs from `a`, in spaces, and whether it looks like alignment.
fn spaces_diff(a: &[u16], a_length: usize, b: &[u16], b_length: usize) -> (usize, bool) {
    let space = u16::from(b' ');
    let common = a[..a_length].iter().zip(&b[..b_length]).take_while(|(x, y)| x == y).count();
    let count = |s: &[u16]| (s.iter().filter(|&&c| c == space).count(), s.iter().filter(|&&c| c != space).count());
    let (a_spaces, a_tabs) = count(&a[common..a_length]);
    let (b_spaces, b_tabs) = count(&b[common..b_length]);
    if (a_spaces > 0 && a_tabs > 0) || (b_spaces > 0 && b_tabs > 0) {
        return (0, false);
    }
    let tabs_diff = a_tabs.abs_diff(b_tabs);
    let spaces = a_spaces.abs_diff(b_spaces);
    if tabs_diff == 0 {
        // `b` lines up with a space in `a` after a line ending in a comma: alignment, not indentation.
        let looks_like_alignment = spaces > 0
            && b_spaces >= 1
            && b_spaces - 1 < a.len()
            && b_spaces < b.len()
            && b[b_spaces] != space
            && a[b_spaces - 1] == space
            && a.last() == Some(&u16::from(b','));
        return (spaces, looks_like_alignment);
    }
    if spaces % tabs_diff == 0 {
        return (spaces / tabs_diff, false);
    }
    (0, false)
}
