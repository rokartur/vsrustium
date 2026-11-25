//! smartSnippetInserter.ts: where Define Keybinding inserts its rule in keybindings.json, and the comma or brackets
//! around it. Positions are the port's 0-based lines and char columns; offsets are the scanner's bytes.

use config::json::{Scanner, SyntaxKind};

use crate::editor::Position;

#[derive(Debug, PartialEq)]
pub struct InsertSnippet {
    pub position: Position,
    pub prepend: &'static str,
    pub append: &'static str,
}

/// `<INVALID> [ <BEFORE_OBJECT> { <INVALID> } <AFTER_OBJECT>, <BEFORE_OBJECT> { <INVALID> } <AFTER_OBJECT> ] <INVALID>`
#[derive(Clone, Copy, PartialEq)]
enum State {
    Invalid,
    AfterObject,
    BeforeObject,
}

/// `SmartSnippetInserter.insertSnippet`: the nearest place at or after `position` between two array elements of
/// the top-level array; none puts a new array at the end.
pub fn insert_snippet(lines: &[String], eol: &str, position: Position) -> InsertSnippet {
    let text = lines.join(eol);
    let line = &lines[position.line];
    let column = line.char_indices().nth(position.column).map_or(line.len(), |(byte, _)| byte);
    let desired: usize = lines[..position.line].iter().map(|line| line.len() + eol.len()).sum::<usize>() + column;

    let mut scanner = Scanner::new(&text, false);
    let (mut array_level, mut object_level) = (0i32, 0i32);
    let mut current_state = State::Invalid;
    let mut last_valid: Option<(usize, State)> = None;
    while scanner.scan() != SyntaxKind::Eof {
        let current = scanner.position();
        let kind = scanner.token();
        let (good_kind, state) = match kind {
            SyntaxKind::OpenBracketToken => {
                array_level += 1;
                (true, Some(State::BeforeObject))
            }
            SyntaxKind::CloseBracketToken => {
                array_level -= 1;
                (true, Some(State::Invalid))
            }
            SyntaxKind::CommaToken => (true, Some(State::BeforeObject)),
            SyntaxKind::OpenBraceToken => {
                object_level += 1;
                (true, Some(State::Invalid))
            }
            SyntaxKind::CloseBraceToken => {
                object_level -= 1;
                (true, Some(State::AfterObject))
            }
            SyntaxKind::Trivia | SyntaxKind::LineBreakTrivia => (true, None),
            _ => (false, None),
        };
        // `checkRangeStatus`.
        if let Some(state) = state {
            if state != State::Invalid && array_level == 1 && object_level == 0 {
                current_state = state;
                last_valid = Some((current, state));
            } else if current_state != State::Invalid {
                current_state = State::Invalid;
                let last_state = last_valid.map_or(State::Invalid, |(_, state)| state);
                last_valid = Some((scanner.token_offset(), last_state));
            }
        }

        if current < desired || (current_state == State::Invalid && last_valid.is_none()) {
            continue;
        }
        let (accept_position, accept_state) = match current_state {
            State::Invalid => last_valid.expect("checked above"),
            state => (if good_kind { current } else { scanner.token_offset() }, state),
        };
        if accept_state == State::AfterObject {
            return InsertSnippet { position: offset_to_position(lines, eol, accept_position), prepend: ",", append: "" };
        }
        scanner.set_position(accept_position);
        let append = if has_open_brace(&mut scanner) { "," } else { "" };
        return InsertSnippet { position: offset_to_position(lines, eol, accept_position), prepend: "", append };
    }

    let last = lines.len() - 1;
    InsertSnippet { position: Position::new(last, lines[last].chars().count()), prepend: "\n[", append: "]" }
}

fn has_open_brace(scanner: &mut Scanner) -> bool {
    while scanner.scan() != SyntaxKind::Eof {
        if scanner.token() == SyntaxKind::OpenBraceToken {
            return true;
        }
    }
    false
}

pub fn offset_to_position(lines: &[String], eol: &str, offset: usize) -> Position {
    let mut before_line = 0;
    for (index, line) in lines.iter().enumerate() {
        let after_line = before_line + line.len() + eol.len();
        if after_line > offset {
            let column = (offset - before_line).min(line.len());
            return Position::new(index, line[..column].chars().count());
        }
        before_line = after_line;
    }
    let last = lines.len() - 1;
    Position::new(last, lines[last].chars().count())
}

/// smartSnippetInserter.test.ts, with upstream's 1-based positions.
#[cfg(test)]
mod tests {
    use super::*;

    /// One upstream case: desired position, expected position, prepend, append.
    type Case = ((usize, usize), (usize, usize), &'static str, &'static str);

    fn check(text: &[&str], cases: &[Case]) {
        let lines: Vec<String> = if text.is_empty() { vec![String::new()] } else { text.iter().map(|line| line.to_string()).collect() };
        for &(desired, expected, prepend, append) in cases {
            let actual = insert_snippet(&lines, "\n", Position::new(desired.0 - 1, desired.1 - 1));
            let expected = InsertSnippet { position: Position::new(expected.0 - 1, expected.1 - 1), prepend, append };
            assert_eq!(actual, expected, "{text:?} at {desired:?}");
        }
    }

    #[test]
    fn empty_text() {
        check(&[], &[((1, 1), (1, 1), "\n[", "]")]);
        check(&[" "], &[((1, 1), (1, 2), "\n[", "]"), ((1, 2), (1, 2), "\n[", "]")]);
        check(&["// just some text"], &[((1, 1), (1, 18), "\n[", "]"), ((1, 18), (1, 18), "\n[", "]")]);
        check(&["// just some text", ""], &[((1, 1), (2, 1), "\n[", "]"), ((1, 18), (2, 1), "\n[", "]"), ((2, 1), (2, 1), "\n[", "]")]);
    }

    #[test]
    fn empty_array_1() {
        check(
            &["// just some text", "[]"],
            &[((1, 1), (2, 2), "", ""), ((2, 1), (2, 2), "", ""), ((2, 2), (2, 2), "", ""), ((2, 3), (2, 2), "", "")],
        );
    }

    #[test]
    fn empty_array_2() {
        check(
            &["// just some text", "[", "]"],
            &[
                ((1, 1), (2, 2), "", ""),
                ((2, 1), (2, 2), "", ""),
                ((2, 2), (2, 2), "", ""),
                ((3, 1), (3, 1), "", ""),
                ((3, 2), (3, 1), "", ""),
            ],
        );
    }

    #[test]
    fn empty_array_3() {
        check(
            &["// just some text", "[", "// just some text", "]"],
            &[
                ((1, 1), (2, 2), "", ""),
                ((2, 1), (2, 2), "", ""),
                ((2, 2), (2, 2), "", ""),
                ((3, 1), (3, 1), "", ""),
                ((3, 2), (3, 1), "", ""),
                ((4, 1), (4, 1), "", ""),
                ((4, 2), (4, 1), "", ""),
            ],
        );
    }

    #[test]
    fn one_element_array_1() {
        check(
            &["// just some text", "[", "{}", "]"],
            &[
                ((1, 1), (2, 2), "", ","),
                ((2, 1), (2, 2), "", ","),
                ((2, 2), (2, 2), "", ","),
                ((3, 1), (3, 1), "", ","),
                ((3, 2), (3, 1), "", ","),
                ((3, 3), (3, 3), ",", ""),
                ((4, 1), (4, 1), ",", ""),
                ((4, 2), (4, 1), ",", ""),
            ],
        );
    }

    #[test]
    fn two_elements_array_1() {
        check(
            &["// just some text", "[", "{},", "{}", "]"],
            &[
                ((1, 1), (2, 2), "", ","),
                ((2, 1), (2, 2), "", ","),
                ((2, 2), (2, 2), "", ","),
                ((3, 1), (3, 1), "", ","),
                ((3, 2), (3, 1), "", ","),
                ((3, 3), (3, 3), ",", ""),
                ((3, 4), (3, 4), "", ","),
                ((4, 1), (4, 1), "", ","),
                ((4, 2), (4, 1), "", ","),
                ((4, 3), (4, 3), ",", ""),
                ((5, 1), (5, 1), ",", ""),
                ((5, 2), (5, 1), ",", ""),
            ],
        );
    }

    #[test]
    fn two_elements_array_2() {
        check(
            &["// just some text", "[", "{},{}", "]"],
            &[
                ((1, 1), (2, 2), "", ","),
                ((2, 1), (2, 2), "", ","),
                ((2, 2), (2, 2), "", ","),
                ((3, 1), (3, 1), "", ","),
                ((3, 2), (3, 1), "", ","),
                ((3, 3), (3, 3), ",", ""),
                ((3, 4), (3, 4), "", ","),
                ((3, 5), (3, 4), "", ","),
                ((3, 6), (3, 6), ",", ""),
                ((4, 1), (4, 1), ",", ""),
                ((4, 2), (4, 1), ",", ""),
            ],
        );
    }
}
