//! base/common/jsonEdit.ts and jsonFormatter.ts: the minimal text edits that set or remove one value in a JSONC
//! document, formatted like the lines around them, so comments and layout of `settings.json` survive.

use serde_json::Value;

use crate::json::{NodeType, ParseOptions, Scanner, ScanError, Segment, SyntaxKind, parse_tree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub offset: usize,
    pub length: usize,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct FormattingOptions {
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub eol: String,
}

/// `removeProperty(text, path, formattingOptions)`.
pub fn remove_property(text: &str, path: &[Segment], options: &FormattingOptions) -> Vec<Edit> {
    set_property(text, path, None, options)
}

/// `setProperty(text, path, value, formattingOptions)`; `None` removes. An index of `usize::MAX` appends (upstream's -1).
pub fn set_property(text: &str, path: &[Segment], value: Option<&Value>, options: &FormattingOptions) -> Vec<Edit> {
    let tree = parse_tree(text, &mut Vec::new(), ParseOptions::default());
    let mut path = path.to_vec();
    let mut value = value.cloned();
    let mut parent = None;
    let mut last_segment = None;
    while let Some(segment) = path.pop() {
        parent = tree.find(tree.root, &path);
        last_segment = Some(segment.clone());
        match (parent, value.take()) {
            (None, Some(inner)) => {
                value = Some(match segment {
                    Segment::Key(key) => Value::Object([(key, inner)].into_iter().collect()),
                    Segment::Index(_) => Value::Array(vec![inner]),
                });
            }
            (_, kept) => {
                value = kept;
                break;
            }
        }
    }

    let Some(parent) = parent else {
        let Some(value) = value else { return Vec::new() };
        let (offset, length) = tree.root.map_or((0, 0), |root| (tree.node(root).offset, tree.node(root).length));
        return with_formatting(text, Edit { offset, length, content: stringify(&value) }, options);
    };
    let parent_node = tree.node(parent);
    match (parent_node.kind, last_segment) {
        (NodeType::Object, Some(Segment::Key(key))) => {
            let existing = tree.find(Some(parent), &[Segment::Key(key.clone())]);
            match (existing, value) {
                (Some(existing), None) => {
                    let property = tree.node(existing).parent.expect("a property value has a property node");
                    let property_index = parent_node.children.iter().position(|&child| child == property).unwrap();
                    let property_node = tree.node(property);
                    let mut remove_end = property_node.offset + property_node.length;
                    let remove_begin;
                    if property_index > 0 {
                        let previous = tree.node(parent_node.children[property_index - 1]);
                        remove_begin = previous.offset + previous.length;
                    } else {
                        remove_begin = parent_node.offset + 1;
                        if parent_node.children.len() > 1 {
                            remove_end = tree.node(parent_node.children[1]).offset;
                        } else {
                            let mut scanner = Scanner::new(text, true);
                            scanner.set_position(remove_end);
                            if scanner.scan() == SyntaxKind::CommaToken {
                                remove_end = scanner.position();
                            }
                        }
                    }
                    with_formatting(text, Edit { offset: remove_begin, length: remove_end - remove_begin, content: String::new() }, options)
                }
                (Some(existing), Some(value)) => {
                    let node = tree.node(existing);
                    with_formatting(text, Edit { offset: node.offset, length: node.length, content: stringify(&value) }, options)
                }
                (None, None) => Vec::new(),
                (None, Some(value)) => {
                    let new_property = format!("{}: {}", stringify(&Value::String(key)), stringify(&value));
                    let edit = if let Some(&previous) = parent_node.children.last() {
                        let previous = tree.node(previous);
                        Edit { offset: previous.offset + previous.length, length: 0, content: format!(",{new_property}") }
                    } else {
                        Edit { offset: parent_node.offset + 1, length: 0, content: new_property }
                    };
                    with_formatting(text, edit, options)
                }
            }
        }
        (NodeType::Array, Some(Segment::Index(index))) => {
            let children = &parent_node.children;
            let edit = if let Some(value) = value {
                let new_property = stringify(&value);
                if children.is_empty() || index == 0 {
                    let content = if children.is_empty() { new_property } else { format!("{new_property},") };
                    Edit { offset: parent_node.offset + 1, length: 0, content }
                } else {
                    let previous = tree.node(children[index.min(children.len()) - 1]);
                    Edit { offset: previous.offset + previous.length, length: 0, content: format!(",{new_property}") }
                }
            } else if children.len() == 1 {
                Edit { offset: parent_node.offset + 1, length: parent_node.length - 2, content: String::new() }
            } else if children.len() - 1 == index {
                let previous = tree.node(children[index - 1]);
                let offset = previous.offset + previous.length;
                Edit { offset, length: parent_node.offset + parent_node.length - 1 - offset, content: String::new() }
            } else {
                let to_remove = tree.node(children[index]);
                Edit { offset: to_remove.offset, length: tree.node(children[index + 1]).offset - to_remove.offset, content: String::new() }
            };
            with_formatting(text, edit, options)
        }
        (kind, segment) => panic!("Can not add {segment:?} to parent of type {kind:?}"),
    }
}

/// `JSON.stringify(value)`: compact, keys in insertion order.
fn stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

fn with_formatting(text: &str, edit: Edit, options: &FormattingOptions) -> Vec<Edit> {
    let mut new_text = apply_edit(text, &edit);
    let mut begin = edit.offset;
    let mut end = edit.offset + edit.content.len();
    if edit.length == 0 || edit.content.is_empty() {
        while begin > 0 && !is_eol(&new_text, begin - 1) {
            begin -= 1;
        }
        while end < new_text.len() && !is_eol(&new_text, end) {
            end += 1;
        }
    }
    let edits = format(&new_text, Some((begin, end - begin)), options);
    for edit in edits.iter().rev() {
        new_text = apply_edit(&new_text, edit);
        begin = begin.min(edit.offset);
        end = end.max(edit.offset + edit.length);
        end = end + edit.content.len() - edit.length;
    }
    let length = text.len() - (new_text.len() - end) - begin;
    vec![Edit { offset: begin, length, content: new_text[begin..end].to_owned() }]
}

pub fn apply_edit(text: &str, edit: &Edit) -> String {
    format!("{}{}{}", &text[..edit.offset], edit.content, &text[edit.offset + edit.length..])
}

/// `applyEdits`; panics on overlapping edits as upstream throws.
pub fn apply_edits(text: &str, edits: &[Edit]) -> String {
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|edit| (edit.offset, edit.length));
    let mut text = text.to_owned();
    let mut last_modified = text.len();
    for edit in sorted.iter().rev() {
        assert!(edit.offset + edit.length <= last_modified, "Overlapping edit");
        text = apply_edit(&text, edit);
        last_modified = edit.offset;
    }
    text
}

fn is_eol(text: &str, offset: usize) -> bool {
    matches!(text.as_bytes().get(offset), Some(b'\r' | b'\n'))
}

/// `getEOL(options, text)`: the first line break of the text, else the option.
pub fn get_eol(options: &FormattingOptions, text: &str) -> String {
    match text.find(['\r', '\n']) {
        Some(index) if text[index..].starts_with("\r\n") => "\r\n".into(),
        Some(index) => text[index..index + 1].into(),
        None => options.eol.clone(),
    }
}

/// `format(documentText, range, options)`; `range` is (offset, length).
pub fn format(document: &str, range: Option<(usize, usize)>, options: &FormattingOptions) -> Vec<Edit> {
    let (format_start, format_text, range_start, range_end, initial_indent_level) = match range {
        Some((offset, length)) => {
            let range_end = offset + length;
            let mut start = offset;
            while start > 0 && !is_eol(document, start - 1) {
                start -= 1;
            }
            let mut end = range_end;
            while end < document.len() && !is_eol(document, end) {
                end += 1;
            }
            let format_text = &document[start..end];
            (start, format_text, offset, range_end, compute_indent_level(format_text, options))
        }
        None => (0, document, 0, document.len(), 0),
    };
    let eol = get_eol(options, document);
    let indent_value = if options.insert_spaces { " ".repeat(if options.tab_size == 0 { 4 } else { options.tab_size }) } else { "\t".into() };
    let mut formatter = Formatter {
        document,
        scanner: Scanner::new(format_text, false),
        format_start,
        range_start,
        range_end,
        line_break: false,
        has_error: false,
        edits: Vec::new(),
    };
    // Signed like upstream's: a stray closing bracket takes it below zero.
    let mut indent_level: isize = 0;
    let new_line_and_indent = |level: isize| format!("{eol}{}", indent_value.repeat((initial_indent_level as isize + level).max(0) as usize));

    let mut first_token = formatter.scan_next();
    if first_token != SyntaxKind::Eof {
        let first_token_start = formatter.scanner.token_offset() + format_start;
        formatter.add_edit(indent_value.repeat(initial_indent_level), format_start, first_token_start);
    }
    while first_token != SyntaxKind::Eof {
        let mut first_token_end = formatter.token_end();
        let mut second_token = formatter.scan_next();
        let mut replace = String::new();
        while !formatter.line_break && matches!(second_token, SyntaxKind::LineCommentTrivia | SyntaxKind::BlockCommentTrivia) {
            let comment_start = formatter.scanner.token_offset() + format_start;
            formatter.add_edit(" ".into(), first_token_end, comment_start);
            first_token_end = formatter.token_end();
            replace = if second_token == SyntaxKind::LineCommentTrivia { new_line_and_indent(indent_level) } else { String::new() };
            second_token = formatter.scan_next();
        }
        if second_token == SyntaxKind::CloseBraceToken {
            if first_token != SyntaxKind::OpenBraceToken {
                indent_level -= 1;
                replace = new_line_and_indent(indent_level);
            }
        } else if second_token == SyntaxKind::CloseBracketToken {
            if first_token != SyntaxKind::OpenBracketToken {
                indent_level -= 1;
                replace = new_line_and_indent(indent_level);
            }
        } else {
            match first_token {
                SyntaxKind::OpenBracketToken | SyntaxKind::OpenBraceToken => {
                    indent_level += 1;
                    replace = new_line_and_indent(indent_level);
                }
                SyntaxKind::CommaToken | SyntaxKind::LineCommentTrivia => replace = new_line_and_indent(indent_level),
                SyntaxKind::BlockCommentTrivia => replace = if formatter.line_break { new_line_and_indent(indent_level) } else { " ".into() },
                SyntaxKind::ColonToken => replace = " ".into(),
                SyntaxKind::StringLiteral if second_token == SyntaxKind::ColonToken => replace = String::new(),
                SyntaxKind::StringLiteral
                | SyntaxKind::NullKeyword
                | SyntaxKind::TrueKeyword
                | SyntaxKind::FalseKeyword
                | SyntaxKind::NumericLiteral
                | SyntaxKind::CloseBraceToken
                | SyntaxKind::CloseBracketToken => {
                    if matches!(second_token, SyntaxKind::LineCommentTrivia | SyntaxKind::BlockCommentTrivia) {
                        replace = " ".into();
                    } else if !matches!(second_token, SyntaxKind::CommaToken | SyntaxKind::Eof) {
                        formatter.has_error = true;
                    }
                }
                SyntaxKind::Unknown => formatter.has_error = true,
                _ => {}
            }
            if formatter.line_break && matches!(second_token, SyntaxKind::LineCommentTrivia | SyntaxKind::BlockCommentTrivia) {
                replace = new_line_and_indent(indent_level);
            }
        }
        let second_token_start = formatter.scanner.token_offset() + format_start;
        formatter.add_edit(replace, first_token_end, second_token_start);
        first_token = second_token;
    }
    formatter.edits
}

struct Formatter<'a> {
    document: &'a str,
    scanner: Scanner<'a>,
    format_start: usize,
    range_start: usize,
    range_end: usize,
    line_break: bool,
    has_error: bool,
    edits: Vec<Edit>,
}

impl Formatter<'_> {
    fn scan_next(&mut self) -> SyntaxKind {
        let mut token = self.scanner.scan();
        self.line_break = false;
        while matches!(token, SyntaxKind::Trivia | SyntaxKind::LineBreakTrivia) {
            self.line_break |= token == SyntaxKind::LineBreakTrivia;
            token = self.scanner.scan();
        }
        self.has_error = token == SyntaxKind::Unknown || self.scanner.token_error() != ScanError::None;
        token
    }

    fn token_end(&self) -> usize {
        self.scanner.token_offset() + self.scanner.token_length() + self.format_start
    }

    fn add_edit(&mut self, text: String, start: usize, end: usize) {
        if !self.has_error && start < self.range_end && end > self.range_start && self.document[start..end] != text {
            self.edits.push(Edit { offset: start, length: end - start, content: text });
        }
    }
}

fn compute_indent_level(content: &str, options: &FormattingOptions) -> usize {
    let tab_size = if options.tab_size == 0 { 4 } else { options.tab_size };
    let mut chars = 0;
    for ch in content.chars() {
        match ch {
            ' ' => chars += 1,
            '\t' => chars += tab_size,
            _ => break,
        }
    }
    chars / tab_size
}

/// base/test/common/jsonEdit.test.ts. The three `getInsertionIndex` cases are left out: settings edits never pass one.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(key: &str) -> Segment {
        Segment::Key(key.into())
    }

    fn options() -> FormattingOptions {
        FormattingOptions { insert_spaces: true, tab_size: 2, eol: "\n".into() }
    }

    fn assert_edit(content: &str, edits: &[Edit], expected: &str) {
        let mut content = content.to_owned();
        let mut last_edit_offset = content.len();
        for edit in edits.iter().rev() {
            assert!(edit.offset + edit.length <= content.len());
            assert!(last_edit_offset >= edit.offset + edit.length, "edits are ordered");
            last_edit_offset = edit.offset;
            content.replace_range(edit.offset..edit.offset + edit.length, &edit.content);
        }
        assert_eq!(content, expected);
    }

    fn set(content: &str, path: &[Segment], value: Option<Value>, expected: &str) {
        assert_edit(content, &set_property(content, path, value.as_ref(), &options()), expected);
    }

    #[test]
    fn set_property_replaces() {
        set("{\n  \"x\": \"y\"\n}", &[key("x")], Some(json!("bar")), "{\n  \"x\": \"bar\"\n}");
        set("true", &[], Some(json!("bar")), "\"bar\"");
        set("{\n  \"x\": \"y\"\n}", &[key("x")], Some(json!({ "key": true })), "{\n  \"x\": {\n    \"key\": true\n  }\n}");
        set("{\n  \"a\": \"b\",  \"x\": \"y\"\n}", &[key("a")], Some(json!(null)), "{\n  \"a\": null,  \"x\": \"y\"\n}");
    }

    #[test]
    fn insert_property() {
        set("{}", &[key("foo")], Some(json!("bar")), "{\n  \"foo\": \"bar\"\n}");
        set("{}", &[key("foo"), key("foo2")], Some(json!("bar")), "{\n  \"foo\": {\n    \"foo2\": \"bar\"\n  }\n}");
        set("{\n}", &[key("foo")], Some(json!("bar")), "{\n  \"foo\": \"bar\"\n}");
        set("  {\n  }", &[key("foo")], Some(json!("bar")), "  {\n    \"foo\": \"bar\"\n  }");
        set("{\n  \"x\": \"y\"\n}", &[key("foo")], Some(json!("bar")), "{\n  \"x\": \"y\",\n  \"foo\": \"bar\"\n}");
        set("{\n  \"x\": \"y\"\n}", &[key("e")], Some(json!("null")), "{\n  \"x\": \"y\",\n  \"e\": \"null\"\n}");
        set("{\n  \"x\": \"y\"\n}", &[key("x")], Some(json!("bar")), "{\n  \"x\": \"bar\"\n}");
        let content = "{\n  \"x\": {\n    \"a\": 1,\n    \"b\": true\n  }\n}\n";
        set(content, &[key("x")], Some(json!("bar")), "{\n  \"x\": \"bar\"\n}\n");
        set(content, &[key("x"), key("b")], Some(json!("bar")), "{\n  \"x\": {\n    \"a\": 1,\n    \"b\": \"bar\"\n  }\n}\n");
        set(content, &[key("c")], Some(json!("bar")), "{\n  \"x\": {\n    \"a\": 1,\n    \"b\": true\n  },\n  \"c\": \"bar\"\n}\n");
        set("{\n  \"a\": [\n    {\n    } \n  ]  \n}", &[key("foo")], Some(json!("bar")), "{\n  \"a\": [\n    {\n    } \n  ],\n  \"foo\": \"bar\"\n}");
        set("", &[key("foo"), Segment::Index(0)], Some(json!("bar")), "{\n  \"foo\": [\n    \"bar\"\n  ]\n}");
        set("//comment", &[key("foo"), Segment::Index(0)], Some(json!("bar")), "{\n  \"foo\": [\n    \"bar\"\n  ]\n} //comment");
    }

    #[test]
    fn remove_property_keeps_the_rest() {
        let remove = |content: &str, path: &[Segment], expected: &str| assert_edit(content, &remove_property(content, path, &options()), expected);
        remove("{\n  \"x\": \"y\"\n}", &[key("x")], "{\n}");
        remove("{\n  \"x\": \"y\", \"a\": []\n}", &[key("x")], "{\n  \"a\": []\n}");
        remove("{\n  \"x\": \"y\", \"a\": []\n}", &[key("a")], "{\n  \"x\": \"y\"\n}");
        remove("{\n  \"x\": \"y\",\n}", &[key("x")], "{\n}");
        remove("{\n  \"x\": \"y\" /* x */, // trailing\n}", &[key("x")], "{ // trailing\n}");
        remove("{\n  \"s\": {\n    \"x\": \"y\",\n  }\n}", &[key("s"), key("x")], "{\n  \"s\": {\n  }\n}");
    }

    #[test]
    fn array_items() {
        let at = Segment::Index;
        set("[\n  2,\n  3\n]", &[at(0)], Some(json!(1)), "[\n  1,\n  2,\n  3\n]");
        set("[\n]", &[at(0)], Some(json!(1)), "[\n  1\n]");
        set("[\n  1,\n  3\n]", &[at(1)], Some(json!(2)), "[\n  1,\n  2,\n  3\n]");
        set("[\n]", &[at(1)], Some(json!(1)), "[\n  1\n]");
        set("[\n  1,\n  2\n]", &[at(2)], Some(json!(3)), "[\n  1,\n  2,\n  3\n]");
        set("[\n]", &[at(usize::MAX)], Some(json!("bar")), "[\n  \"bar\"\n]");
        set("[\n  1,\n  2\n]", &[at(usize::MAX)], Some(json!("bar")), "[\n  1,\n  2,\n  \"bar\"\n]");
        set("[\n  1\n]", &[at(0)], None, "[]");
        set("[\n  1,\n  2,\n  3\n]", &[at(1)], None, "[\n  1,\n  3\n]");
        set("[\n  1,\n  2,\n  \"bar\"\n]", &[at(2)], None, "[\n  1,\n  2\n]");
        set("[\n  1,\n  \"foo\",\n  \"bar\",\n]", &[at(2)], None, "[\n  1,\n  \"foo\"\n]");
        set("// This is a comment\n[\n  1,\n  \"foo\",\n  \"bar\"\n]", &[at(2)], None, "// This is a comment\n[\n  1,\n  \"foo\"\n]");
        set("[1,2]", &[at(1)], None, "[\n  1\n]");
    }
}
