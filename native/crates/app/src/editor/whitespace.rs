//! Which spaces and tabs `editor.renderWhitespace` marks (viewParts/whitespace/whitespace.ts `_applyRenderWhitespace`).

use std::ops::Range;

/// The char indices of the marked spaces and tabs among the first `len` chars of `line`. `selections` holds the
/// selected char ranges on the line, which `selection` mode needs. The port never wraps, so no line continues.
pub fn whitespace_marks(line: &str, mode: &str, selections: &[Range<usize>], len: usize) -> Vec<usize> {
    let only_boundary = mode == "boundary";
    let only_trailing = mode == "trailing";
    if !matches!(mode, "all" | "boundary" | "selection" | "trailing") || (mode == "selection" && selections.is_empty()) {
        return Vec::new();
    }
    let chars: Vec<char> = line.chars().collect();
    let is_whitespace = |c: char| c == ' ' || c == '\t';
    let first_non_whitespace = chars.iter().position(|&c| !is_whitespace(c));
    let last_non_whitespace = chars.iter().rposition(|&c| !is_whitespace(c));
    let mut marks = Vec::new();
    for index in 0..len.min(chars.len()) {
        let c = chars[index];
        if !is_whitespace(c) {
            continue;
        }
        let inside = first_non_whitespace.is_some_and(|first| first <= index) && last_non_whitespace.is_some_and(|last| index <= last);
        if only_trailing && first_non_whitespace.is_some() && last_non_whitespace.is_some_and(|last| index <= last) {
            continue;
        }
        if only_boundary && inside && c == ' ' {
            let previous = index.checked_sub(1).map(|previous| chars[previous]);
            let next = chars.get(index + 1).filter(|_| index + 1 < len).copied();
            if previous != Some(' ') && next != Some(' ') {
                continue;
            }
        }
        if mode == "selection" && !selections.iter().any(|selection| selection.contains(&index)) {
            continue;
        }
        marks.push(index);
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::whitespace_marks;

    #[test]
    fn each_mode_marks_its_spaces_and_tabs() {
        let line = "\t a  b c ";
        let len = line.len();
        assert_eq!(whitespace_marks(line, "none", &[], len), Vec::<usize>::new());
        assert_eq!(whitespace_marks(line, "all", &[], len), [0, 1, 3, 4, 6, 8]);
        // A single space between words is no boundary.
        assert_eq!(whitespace_marks(line, "boundary", &[], len), [0, 1, 3, 4, 8]);
        assert_eq!(whitespace_marks(line, "trailing", &[], len), [8]);
        assert_eq!(whitespace_marks("  ", "trailing", &[], 2), [0, 1]);
    }

    #[test]
    fn selection_mode_marks_only_selected_whitespace_and_nothing_without_a_selection() {
        let line = "a b  c";
        assert_eq!(whitespace_marks(line, "selection", &[], line.len()), Vec::<usize>::new());
        assert_eq!(whitespace_marks(line, "selection", &[2..6], line.len()), [3, 4]);
        assert_eq!(whitespace_marks(line, "selection", &[0..2, 4..5], line.len()), [1, 4]);
    }

    #[test]
    fn whitespace_past_the_rendered_length_is_not_marked() {
        assert_eq!(whitespace_marks("a b c", "all", &[], 2), [1]);
    }
}
