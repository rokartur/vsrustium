//! The text a snippet inserts and its first tab stop (snippetParser.ts, snippetSession.ts `adjustWhitespace`),
//! for `$tryInsertSnippet`.
// shortcut: no snippet session (Tab through the stops, linked placeholders, choice list) and no transforms; the
// session is the `editor/contrib/snippet` row of milestone 11, which takes this parser over.

use std::collections::HashMap;
use std::ops::Range;

use super::Indentation;

#[derive(Clone, Debug, PartialEq)]
enum Marker {
    Text(String),
    /// A choice option: inserted as is, never re-indented.
    Choice(String),
    Placeholder { index: u32, children: Vec<Marker> },
    Variable { name: String, children: Vec<Marker> },
}

/// What inserting a snippet writes, and the char range in it the cursor selects.
#[derive(Debug, PartialEq)]
pub struct SnippetText {
    pub text: String,
    pub selection: Range<usize>,
}

/// `SnippetParser.parse(template, true)` then `adjustWhitespace` and `resolveVariables`: unknown variables insert their
/// default. `line_indent` is the leading whitespace before the insert position, applied to every following line
/// when `adjust_indentation`.
pub fn expand(template: &str, line_indent: &str, adjust_indentation: bool, indentation: Indentation, variable: impl Fn(&str) -> Option<String>) -> SnippetText {
    let chars: Vec<char> = template.chars().collect();
    let mut position = 0;
    let mut markers = parse(&chars, &mut position, false);
    fill_in_placeholders(&mut markers);
    if has_placeholders(&markers) && !has_final_tabstop(&markers) {
        markers.push(Marker::Placeholder { index: 0, children: Vec::new() });
    }
    let mut out = Output { text: String::new(), length: 0, stops: Vec::new(), line_indent, adjust_indentation, indentation, variable: &variable };
    out.write(&markers);
    // snippetSession.ts `_move(true)`: the lowest index first, the final tab stop last; none puts the cursor at the end.
    let first = out.stops.iter().min_by_key(|(index, _)| if *index == 0 { u32::MAX } else { *index });
    let selection = first.map_or(out.length..out.length, |(_, range)| range.clone());
    SnippetText { text: out.text, selection }
}

fn parse(chars: &[char], position: &mut usize, nested: bool) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut text = String::new();
    while *position < chars.len() {
        let c = chars[*position];
        if nested && c == '}' {
            break;
        }
        if c == '\\' && matches!(chars.get(*position + 1), Some('$' | '}' | '\\')) {
            text.push(chars[*position + 1]);
            *position += 2;
            continue;
        }
        if c == '$' {
            let start = *position;
            if let Some(marker) = parse_dollar(chars, position) {
                if !text.is_empty() {
                    markers.push(Marker::Text(std::mem::take(&mut text)));
                }
                markers.push(marker);
                continue;
            }
            *position = start;
        }
        text.push(c);
        *position += 1;
    }
    if !text.is_empty() {
        markers.push(Marker::Text(text));
    }
    markers
}

/// `$1`, `$name`, `${1}`, `${1:default}`, `${1|one,two|}`, `${name}`, `${name:default}`; `None` leaves the `$` as text.
fn parse_dollar(chars: &[char], position: &mut usize) -> Option<Marker> {
    *position += 1;
    let braced = chars.get(*position) == Some(&'{');
    if braced {
        *position += 1;
    }
    let index = take_while(chars, position, |c| c.is_ascii_digit());
    let name = match index.is_empty() {
        true => take_while(chars, position, |c| c == '_' || c.is_ascii_alphanumeric()),
        false => String::new(),
    };
    if index.is_empty() && !name.starts_with(|c: char| c == '_' || c.is_ascii_alphabetic()) {
        return None;
    }
    let new_marker = |children| match index.parse() {
        Ok(index) => Marker::Placeholder { index, children },
        Err(_) => Marker::Variable { name: name.clone(), children },
    };
    if !braced {
        return Some(new_marker(Vec::new()));
    }
    match chars.get(*position)? {
        '}' => {
            *position += 1;
            Some(new_marker(Vec::new()))
        }
        ':' => {
            *position += 1;
            let children = parse(chars, position, true);
            (chars.get(*position) == Some(&'}')).then(|| {
                *position += 1;
                new_marker(children)
            })
        }
        '|' if !index.is_empty() => {
            *position += 1;
            let options = parse_choice(chars, position)?;
            Some(new_marker(vec![Marker::Choice(options.into_iter().next()?)]))
        }
        _ => None,
    }
}

/// The options of `${1|one,two|}` after the first `|`, with `\,` `\|` and `\\` unescaped.
fn parse_choice(chars: &[char], position: &mut usize) -> Option<Vec<String>> {
    let mut options = vec![String::new()];
    loop {
        match (chars.get(*position)?, chars.get(*position + 1)) {
            ('\\', Some(&next @ (',' | '|' | '\\'))) => {
                options.last_mut()?.push(next);
                *position += 2;
            }
            ('|', Some('}')) => {
                *position += 2;
                return Some(options);
            }
            (',', _) => {
                options.push(String::new());
                *position += 1;
            }
            (&c, _) => {
                options.last_mut()?.push(c);
                *position += 1;
            }
        }
    }
}

fn take_while(chars: &[char], position: &mut usize, accept: impl Fn(char) -> bool) -> String {
    let start = *position;
    while chars.get(*position).is_some_and(|c| accept(*c)) {
        *position += 1;
    }
    chars[start..*position].iter().collect()
}

/// `parseFragment`: the first placeholder of an index with a value gives it to the empty ones of that index.
fn fill_in_placeholders(markers: &mut [Marker]) {
    let mut values = HashMap::new();
    collect_values(markers, &mut values);
    fill(markers, &values);
}

fn collect_values(markers: &[Marker], values: &mut HashMap<u32, Vec<Marker>>) {
    for marker in markers {
        match marker {
            Marker::Placeholder { index, children } => {
                if *index != 0 && !children.is_empty() {
                    values.entry(*index).or_insert_with(|| children.clone());
                }
                collect_values(children, values);
            }
            Marker::Variable { children, .. } => collect_values(children, values),
            Marker::Text(_) | Marker::Choice(_) => {}
        }
    }
}

fn fill(markers: &mut [Marker], values: &HashMap<u32, Vec<Marker>>) {
    for marker in markers {
        match marker {
            Marker::Placeholder { index, children } if children.is_empty() => {
                if let Some(value) = values.get(index) {
                    *children = value.clone();
                }
            }
            Marker::Placeholder { children, .. } | Marker::Variable { children, .. } => fill(children, values),
            Marker::Text(_) | Marker::Choice(_) => {}
        }
    }
}

fn has_placeholders(markers: &[Marker]) -> bool {
    markers.iter().any(|marker| match marker {
        Marker::Placeholder { .. } => true,
        Marker::Variable { children, .. } => has_placeholders(children),
        Marker::Text(_) | Marker::Choice(_) => false,
    })
}

fn has_final_tabstop(markers: &[Marker]) -> bool {
    markers.iter().any(|marker| match marker {
        Marker::Placeholder { index: 0, .. } => true,
        Marker::Placeholder { children, .. } | Marker::Variable { children, .. } => has_final_tabstop(children),
        Marker::Text(_) | Marker::Choice(_) => false,
    })
}

struct Output<'a> {
    text: String,
    /// The length of `text` in chars.
    length: usize,
    stops: Vec<(u32, Range<usize>)>,
    line_indent: &'a str,
    adjust_indentation: bool,
    indentation: Indentation,
    variable: &'a dyn Fn(&str) -> Option<String>,
}

impl Output<'_> {
    fn write(&mut self, markers: &[Marker]) {
        for marker in markers {
            match marker {
                Marker::Text(text) => {
                    let text = self.adjust_whitespace(text);
                    self.push(&text);
                }
                Marker::Choice(text) => self.push(text),
                Marker::Placeholder { index, children } => {
                    let start = self.length;
                    self.write(children);
                    self.stops.push((*index, start..self.length));
                }
                Marker::Variable { name, children } => match (self.variable)(name) {
                    Some(value) => self.push(&value),
                    None => self.write(children),
                },
            }
        }
    }

    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.length += text.chars().count();
    }

    /// `adjustWhitespace`: every line after the first of a text marker takes the insert line's indentation in front
    /// of its own, normalized to the model's tabs or spaces. Line breaks become "\n".
    fn adjust_whitespace(&self, text: &str) -> String {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if !self.adjust_indentation {
            return normalized;
        }
        let mut lines = normalized.split('\n');
        let mut adjusted = lines.next().unwrap_or_default().to_string();
        for line in lines {
            let own_indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            adjusted.push('\n');
            adjusted.push_str(&normalize_indentation(&format!("{}{own_indent}", self.line_indent), self.indentation));
            adjusted.push_str(&line[own_indent.len()..]);
        }
        adjusted
    }
}

/// textModel.ts `normalizeIndentation`: leading whitespace as its visible width in tabs or spaces.
pub fn normalize_indentation(whitespace: &str, indentation: Indentation) -> String {
    let tab_size = indentation.tab_size;
    let width = whitespace.chars().fold(0, |width, c| if c == '\t' { width + tab_size - width % tab_size } else { width + 1 });
    match indentation.insert_spaces {
        true => " ".repeat(width),
        false => "\t".repeat(width / tab_size) + &" ".repeat(width % tab_size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACES: Indentation = Indentation { tab_size: 4, indent_size: None, insert_spaces: true };

    fn plain(template: &str) -> SnippetText {
        expand(template, "", true, SPACES, |name| (name == "CLIPBOARD").then(|| "clip".to_string()))
    }

    #[test]
    fn placeholders_insert_their_text_and_the_lowest_index_is_selected() {
        assert_eq!(plain("This is a $1${2:placeholder} snippet"), SnippetText { text: "This is a placeholder snippet".into(), selection: 10..10 });
        assert_eq!(plain("a ${2:two} ${1:one}"), SnippetText { text: "a two one".into(), selection: 6..9 });
        assert_eq!(plain("${1:x} and $1"), SnippetText { text: "x and x".into(), selection: 0..1 });
        assert_eq!(plain("${1|yes,no|}!"), SnippetText { text: "yes!".into(), selection: 0..3 });
    }

    #[test]
    fn plain_text_puts_the_cursor_at_its_end_and_escapes_unescape() {
        assert_eq!(plain("has been"), SnippetText { text: "has been".into(), selection: 8..8 });
        assert_eq!(plain(r"cost \$5 \} \\ $"), SnippetText { text: r"cost $5 } \ $".into(), selection: 13..13 });
        assert_eq!(plain("a$0b"), SnippetText { text: "ab".into(), selection: 1..1 });
    }

    #[test]
    fn variables_resolve_or_insert_their_default() {
        assert_eq!(plain("running: $CLIPBOARD").text, "running: clip");
        assert_eq!(plain("${UNKNOWN:fallback} ${NOPE}").text, "fallback ");
    }

    #[test]
    fn following_lines_take_the_insert_lines_indentation_unless_whitespace_is_kept() {
        let template = "This is line 2\n  This is line 3";
        assert_eq!(expand(template, "  ", false, SPACES, |_| None).text, template);
        assert_eq!(expand(template, "  ", true, SPACES, |_| None).text, "This is line 2\n    This is line 3");
        let tabs = Indentation { tab_size: 4, indent_size: None, insert_spaces: false };
        assert_eq!(expand("a\n\tb", "\t", true, tabs, |_| None).text, "a\n\t\tb");
    }
}
