//! JSON with comments and trailing commas, as base/common/json.ts scans and parses it: the same tokens, the same
//! error recovery and the same error codes. Offsets are UTF-8 byte offsets into the text.

use serde_json::{Map, Number, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanError {
    None,
    UnexpectedEndOfComment,
    UnexpectedEndOfString,
    UnexpectedEndOfNumber,
    InvalidUnicode,
    InvalidEscapeCharacter,
    InvalidCharacter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxKind {
    OpenBraceToken,
    CloseBraceToken,
    OpenBracketToken,
    CloseBracketToken,
    CommaToken,
    ColonToken,
    NullKeyword,
    TrueKeyword,
    FalseKeyword,
    StringLiteral,
    NumericLiteral,
    LineCommentTrivia,
    BlockCommentTrivia,
    LineBreakTrivia,
    Trivia,
    Unknown,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorCode {
    InvalidSymbol = 1,
    InvalidNumberFormat,
    PropertyNameExpected,
    ValueExpected,
    ColonExpected,
    CommaExpected,
    CloseBraceExpected,
    CloseBracketExpected,
    EndOfFileExpected,
    InvalidCommentToken,
    UnexpectedEndOfComment,
    UnexpectedEndOfString,
    UnexpectedEndOfNumber,
    InvalidUnicode,
    InvalidEscapeCharacter,
    InvalidCharacter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub error: ParseErrorCode,
    pub offset: usize,
    pub length: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    pub disallow_comments: bool,
    pub allow_trailing_comma: bool,
    pub allow_empty_content: bool,
}

impl Default for ParseOptions {
    /// `ParseOptions.DEFAULT`.
    fn default() -> Self {
        ParseOptions { disallow_comments: false, allow_trailing_comma: true, allow_empty_content: false }
    }
}

pub struct Scanner<'a> {
    text: &'a str,
    ignore_trivia: bool,
    pos: usize,
    value: String,
    token_offset: usize,
    token: SyntaxKind,
    error: ScanError,
}

impl<'a> Scanner<'a> {
    /// `createScanner(text, ignoreTrivia)`.
    pub fn new(text: &'a str, ignore_trivia: bool) -> Self {
        Scanner { text, ignore_trivia, pos: 0, value: String::new(), token_offset: 0, token: SyntaxKind::Unknown, error: ScanError::None }
    }

    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos;
        self.value.clear();
        self.token_offset = 0;
        self.token = SyntaxKind::Unknown;
        self.error = ScanError::None;
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn token(&self) -> SyntaxKind {
        self.token
    }

    pub fn token_value(&self) -> &str {
        &self.value
    }

    pub fn token_offset(&self) -> usize {
        self.token_offset
    }

    pub fn token_length(&self) -> usize {
        self.pos - self.token_offset
    }

    pub fn token_error(&self) -> ScanError {
        self.error
    }

    pub fn scan(&mut self) -> SyntaxKind {
        loop {
            let token = self.scan_next();
            if !self.ignore_trivia || !matches!(token, SyntaxKind::LineCommentTrivia | SyntaxKind::BlockCommentTrivia | SyntaxKind::LineBreakTrivia | SyntaxKind::Trivia) {
                return token;
            }
        }
    }

    fn char_at(&self, pos: usize) -> Option<char> {
        self.text.get(pos..).and_then(|rest| rest.chars().next())
    }

    fn byte_at(&self, pos: usize) -> Option<u8> {
        self.text.as_bytes().get(pos).copied()
    }

    fn is_digit_at(&self, pos: usize) -> bool {
        self.byte_at(pos).is_some_and(|b| b.is_ascii_digit())
    }

    fn scan_hex_digits(&mut self, count: usize) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            let digit = self.byte_at(self.pos).and_then(|b| (b as char).to_digit(16))?;
            value = value * 16 + digit;
            self.pos += 1;
        }
        Some(value)
    }

    fn scan_number(&mut self) -> String {
        let start = self.pos;
        self.pos += 1;
        if self.byte_at(start) != Some(b'0') {
            while self.is_digit_at(self.pos) {
                self.pos += 1;
            }
        }
        if self.byte_at(self.pos) == Some(b'.') {
            self.pos += 1;
            if !self.is_digit_at(self.pos) {
                self.error = ScanError::UnexpectedEndOfNumber;
                return self.text[start..self.pos].to_owned();
            }
            while self.is_digit_at(self.pos) {
                self.pos += 1;
            }
        }
        let mut end = self.pos;
        if matches!(self.byte_at(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.byte_at(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.is_digit_at(self.pos) {
                while self.is_digit_at(self.pos) {
                    self.pos += 1;
                }
                end = self.pos;
            } else {
                self.error = ScanError::UnexpectedEndOfNumber;
            }
        }
        self.text[start..end].to_owned()
    }

    fn scan_string(&mut self) -> String {
        let mut result = String::new();
        let mut start = self.pos;
        loop {
            let Some(ch) = self.char_at(self.pos) else {
                result.push_str(&self.text[start..self.pos]);
                self.error = ScanError::UnexpectedEndOfString;
                break;
            };
            if ch == '"' {
                result.push_str(&self.text[start..self.pos]);
                self.pos += 1;
                break;
            }
            if ch == '\\' {
                result.push_str(&self.text[start..self.pos]);
                self.pos += 1;
                let Some(escape) = self.char_at(self.pos) else {
                    self.error = ScanError::UnexpectedEndOfString;
                    break;
                };
                self.pos += escape.len_utf8();
                match escape {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    '/' => result.push('/'),
                    'b' => result.push('\u{8}'),
                    'f' => result.push('\u{c}'),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    'u' => match self.scan_hex_digits(4) {
                        Some(unit) => self.push_utf16_unit(&mut result, unit),
                        None => self.error = ScanError::InvalidUnicode,
                    },
                    _ => self.error = ScanError::InvalidEscapeCharacter,
                }
                start = self.pos;
                continue;
            }
            if (ch as u32) <= 0x1f {
                if is_line_break(ch) {
                    result.push_str(&self.text[start..self.pos]);
                    self.error = ScanError::UnexpectedEndOfString;
                    break;
                }
                self.error = ScanError::InvalidCharacter;
            }
            self.pos += ch.len_utf8();
        }
        result
    }

    /// `String.fromCharCode` of one UTF-16 unit: a high surrogate pairs with a following `\uDCxx`; a lone one
    /// becomes U+FFFD, since a Rust string cannot hold it.
    fn push_utf16_unit(&mut self, result: &mut String, unit: u32) {
        if (0xd800..0xdc00).contains(&unit) && self.text[self.pos..].starts_with("\\u") {
            let save = self.pos;
            self.pos += 2;
            if let Some(low) = self.scan_hex_digits(4).filter(|low| (0xdc00..0xe000).contains(low)) {
                result.push(char::from_u32(0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00)).unwrap());
                return;
            }
            self.pos = save;
        }
        result.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
    }

    fn scan_next(&mut self) -> SyntaxKind {
        self.value.clear();
        self.error = ScanError::None;
        self.token_offset = self.pos;
        let Some(ch) = self.char_at(self.pos) else {
            self.token_offset = self.text.len();
            self.token = SyntaxKind::Eof;
            return self.token;
        };
        self.token = self.scan_token(ch);
        self.token
    }

    fn scan_token(&mut self, ch: char) -> SyntaxKind {
        if is_whitespace(ch) {
            let start = self.pos;
            while let Some(ch) = self.char_at(self.pos).filter(|ch| is_whitespace(*ch)) {
                self.pos += ch.len_utf8();
            }
            self.value = self.text[start..self.pos].to_owned();
            return SyntaxKind::Trivia;
        }
        if is_line_break(ch) {
            let start = self.pos;
            self.pos += ch.len_utf8();
            if ch == '\r' && self.byte_at(self.pos) == Some(b'\n') {
                self.pos += 1;
            }
            self.value = self.text[start..self.pos].to_owned();
            return SyntaxKind::LineBreakTrivia;
        }
        let single = |kind| (kind, 1);
        let (kind, width) = match ch {
            '{' => single(SyntaxKind::OpenBraceToken),
            '}' => single(SyntaxKind::CloseBraceToken),
            '[' => single(SyntaxKind::OpenBracketToken),
            ']' => single(SyntaxKind::CloseBracketToken),
            ':' => single(SyntaxKind::ColonToken),
            ',' => single(SyntaxKind::CommaToken),
            '"' => {
                self.pos += 1;
                self.value = self.scan_string();
                return SyntaxKind::StringLiteral;
            }
            '/' => return self.scan_comment(),
            '-' => {
                self.pos += 1;
                if !self.is_digit_at(self.pos) {
                    self.value.push('-');
                    return SyntaxKind::Unknown;
                }
                self.value = format!("-{}", self.scan_number());
                return SyntaxKind::NumericLiteral;
            }
            '0'..='9' => {
                self.value = self.scan_number();
                return SyntaxKind::NumericLiteral;
            }
            _ => return self.scan_word(ch),
        };
        self.pos += width;
        kind
    }

    fn scan_comment(&mut self) -> SyntaxKind {
        let start = self.pos;
        match self.byte_at(self.pos + 1) {
            Some(b'/') => {
                self.pos += 2;
                while let Some(ch) = self.char_at(self.pos).filter(|ch| !is_line_break(*ch)) {
                    self.pos += ch.len_utf8();
                }
                self.value = self.text[start..self.pos].to_owned();
                SyntaxKind::LineCommentTrivia
            }
            Some(b'*') => {
                self.pos += 2;
                match self.text[self.pos..].find("*/") {
                    Some(end) => self.pos += end + 2,
                    None => {
                        self.pos = self.text.len();
                        self.error = ScanError::UnexpectedEndOfComment;
                    }
                }
                self.value = self.text[start..self.pos].to_owned();
                SyntaxKind::BlockCommentTrivia
            }
            _ => {
                self.pos += 1;
                self.value.push('/');
                SyntaxKind::Unknown
            }
        }
    }

    fn scan_word(&mut self, first: char) -> SyntaxKind {
        let start = self.pos;
        while let Some(ch) = self.char_at(self.pos).filter(|ch| is_unknown_content_character(*ch)) {
            self.pos += ch.len_utf8();
        }
        if start == self.pos {
            self.pos += first.len_utf8();
            self.value.push(first);
            return SyntaxKind::Unknown;
        }
        self.value = self.text[start..self.pos].to_owned();
        match self.value.as_str() {
            "true" => SyntaxKind::TrueKeyword,
            "false" => SyntaxKind::FalseKeyword,
            "null" => SyntaxKind::NullKeyword,
            _ => SyntaxKind::Unknown,
        }
    }
}

fn is_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\u{b}' | '\u{c}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200b}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn is_line_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_unknown_content_character(ch: char) -> bool {
    !is_whitespace(ch) && !is_line_break(ch) && !matches!(ch, '}' | ']' | '{' | '[' | '"' | ':' | ',' | '/')
}

/// `JSONVisitor`: every callback gets the offset and length of its token.
#[allow(unused_variables)]
pub trait Visitor {
    fn on_object_begin(&mut self, offset: usize, length: usize) {}
    fn on_object_property(&mut self, name: &str, offset: usize, length: usize) {}
    fn on_object_end(&mut self, offset: usize, length: usize) {}
    fn on_array_begin(&mut self, offset: usize, length: usize) {}
    fn on_array_end(&mut self, offset: usize, length: usize) {}
    fn on_literal_value(&mut self, value: Value, offset: usize, length: usize) {}
    fn on_separator(&mut self, separator: char, offset: usize, length: usize) {}
    fn on_comment(&mut self, offset: usize, length: usize) {}
    fn on_error(&mut self, error: ParseErrorCode, offset: usize, length: usize) {}
}

/// `visit(text, visitor, options)`. Returns false when the text holds no value.
pub fn visit(text: &str, visitor: &mut dyn Visitor, options: ParseOptions) -> bool {
    let mut parser = Parser { scanner: Scanner::new(text, false), visitor, options };
    parser.run()
}

struct Parser<'a, 'v> {
    scanner: Scanner<'a>,
    visitor: &'v mut dyn Visitor,
    options: ParseOptions,
}

impl Parser<'_, '_> {
    fn token(&self) -> SyntaxKind {
        self.scanner.token()
    }

    fn offset(&self) -> (usize, usize) {
        (self.scanner.token_offset(), self.scanner.token_length())
    }

    fn scan_next(&mut self) -> SyntaxKind {
        loop {
            let token = self.scanner.scan();
            match self.scanner.token_error() {
                ScanError::None => {}
                ScanError::InvalidUnicode => self.error(ParseErrorCode::InvalidUnicode, &[], &[]),
                ScanError::InvalidEscapeCharacter => self.error(ParseErrorCode::InvalidEscapeCharacter, &[], &[]),
                ScanError::UnexpectedEndOfNumber => self.error(ParseErrorCode::UnexpectedEndOfNumber, &[], &[]),
                ScanError::UnexpectedEndOfComment => {
                    if !self.options.disallow_comments {
                        self.error(ParseErrorCode::UnexpectedEndOfComment, &[], &[]);
                    }
                }
                ScanError::UnexpectedEndOfString => self.error(ParseErrorCode::UnexpectedEndOfString, &[], &[]),
                ScanError::InvalidCharacter => self.error(ParseErrorCode::InvalidCharacter, &[], &[]),
            }
            match token {
                SyntaxKind::LineCommentTrivia | SyntaxKind::BlockCommentTrivia => {
                    if self.options.disallow_comments {
                        self.error(ParseErrorCode::InvalidCommentToken, &[], &[]);
                    } else {
                        let (offset, length) = self.offset();
                        self.visitor.on_comment(offset, length);
                    }
                }
                SyntaxKind::Unknown => self.error(ParseErrorCode::InvalidSymbol, &[], &[]),
                SyntaxKind::Trivia | SyntaxKind::LineBreakTrivia => {}
                _ => return token,
            }
        }
    }

    fn error(&mut self, error: ParseErrorCode, skip_until_after: &[SyntaxKind], skip_until: &[SyntaxKind]) {
        let (offset, length) = self.offset();
        self.visitor.on_error(error, offset, length);
        if skip_until_after.is_empty() && skip_until.is_empty() {
            return;
        }
        let mut token = self.token();
        while token != SyntaxKind::Eof {
            if skip_until_after.contains(&token) {
                self.scan_next();
                break;
            } else if skip_until.contains(&token) {
                break;
            }
            token = self.scan_next();
        }
    }

    fn parse_string(&mut self, is_value: bool) {
        let value = self.scanner.token_value().to_owned();
        let (offset, length) = self.offset();
        if is_value {
            self.visitor.on_literal_value(Value::String(value), offset, length);
        } else {
            self.visitor.on_object_property(&value, offset, length);
        }
        self.scan_next();
    }

    fn parse_literal(&mut self) -> bool {
        let (offset, length) = self.offset();
        let value = match self.token() {
            SyntaxKind::NumericLiteral => match self.scanner.token_value().parse::<Number>() {
                Ok(number) => Value::Number(number),
                Err(_) => {
                    self.error(ParseErrorCode::InvalidNumberFormat, &[], &[]);
                    Value::from(0)
                }
            },
            SyntaxKind::NullKeyword => Value::Null,
            SyntaxKind::TrueKeyword => Value::Bool(true),
            SyntaxKind::FalseKeyword => Value::Bool(false),
            _ => return false,
        };
        self.visitor.on_literal_value(value, offset, length);
        self.scan_next();
        true
    }

    fn parse_property(&mut self) -> bool {
        const RECOVER: &[SyntaxKind] = &[SyntaxKind::CloseBraceToken, SyntaxKind::CommaToken];
        if self.token() != SyntaxKind::StringLiteral {
            self.error(ParseErrorCode::PropertyNameExpected, &[], RECOVER);
            return false;
        }
        self.parse_string(false);
        if self.token() == SyntaxKind::ColonToken {
            let (offset, length) = self.offset();
            self.visitor.on_separator(':', offset, length);
            self.scan_next();
            if !self.parse_value() {
                self.error(ParseErrorCode::ValueExpected, &[], RECOVER);
            }
        } else {
            self.error(ParseErrorCode::ColonExpected, &[], RECOVER);
        }
        true
    }

    fn parse_object(&mut self) -> bool {
        let (offset, length) = self.offset();
        self.visitor.on_object_begin(offset, length);
        self.scan_next();
        let mut needs_comma = false;
        while !matches!(self.token(), SyntaxKind::CloseBraceToken | SyntaxKind::Eof) {
            if self.token() == SyntaxKind::CommaToken {
                if !needs_comma {
                    self.error(ParseErrorCode::ValueExpected, &[], &[]);
                }
                let (offset, length) = self.offset();
                self.visitor.on_separator(',', offset, length);
                self.scan_next();
                if self.token() == SyntaxKind::CloseBraceToken && self.options.allow_trailing_comma {
                    break;
                }
            } else if needs_comma {
                self.error(ParseErrorCode::CommaExpected, &[], &[]);
            }
            if !self.parse_property() {
                self.error(ParseErrorCode::ValueExpected, &[], &[SyntaxKind::CloseBraceToken, SyntaxKind::CommaToken]);
            }
            needs_comma = true;
        }
        let (offset, length) = self.offset();
        self.visitor.on_object_end(offset, length);
        if self.token() != SyntaxKind::CloseBraceToken {
            self.error(ParseErrorCode::CloseBraceExpected, &[SyntaxKind::CloseBraceToken], &[]);
        } else {
            self.scan_next();
        }
        true
    }

    fn parse_array(&mut self) -> bool {
        let (offset, length) = self.offset();
        self.visitor.on_array_begin(offset, length);
        self.scan_next();
        let mut needs_comma = false;
        while !matches!(self.token(), SyntaxKind::CloseBracketToken | SyntaxKind::Eof) {
            if self.token() == SyntaxKind::CommaToken {
                if !needs_comma {
                    self.error(ParseErrorCode::ValueExpected, &[], &[]);
                }
                let (offset, length) = self.offset();
                self.visitor.on_separator(',', offset, length);
                self.scan_next();
                if self.token() == SyntaxKind::CloseBracketToken && self.options.allow_trailing_comma {
                    break;
                }
            } else if needs_comma {
                self.error(ParseErrorCode::CommaExpected, &[], &[]);
            }
            if !self.parse_value() {
                self.error(ParseErrorCode::ValueExpected, &[], &[SyntaxKind::CloseBracketToken, SyntaxKind::CommaToken]);
            }
            needs_comma = true;
        }
        let (offset, length) = self.offset();
        self.visitor.on_array_end(offset, length);
        if self.token() != SyntaxKind::CloseBracketToken {
            self.error(ParseErrorCode::CloseBracketExpected, &[SyntaxKind::CloseBracketToken], &[]);
        } else {
            self.scan_next();
        }
        true
    }

    fn parse_value(&mut self) -> bool {
        match self.token() {
            SyntaxKind::OpenBracketToken => self.parse_array(),
            SyntaxKind::OpenBraceToken => self.parse_object(),
            SyntaxKind::StringLiteral => {
                self.parse_string(true);
                true
            }
            _ => self.parse_literal(),
        }
    }

    fn run(&mut self) -> bool {
        self.scan_next();
        if self.token() == SyntaxKind::Eof {
            if self.options.allow_empty_content {
                return true;
            }
            self.error(ParseErrorCode::ValueExpected, &[], &[]);
            return false;
        }
        if !self.parse_value() {
            self.error(ParseErrorCode::ValueExpected, &[], &[]);
            return false;
        }
        if self.token() != SyntaxKind::Eof {
            self.error(ParseErrorCode::EndOfFileExpected, &[], &[]);
        }
        true
    }
}

/// `parse(text, errors, options)`: the value the text holds, recovering from errors as far as it can.
/// `None` where upstream returns `undefined` (no value at all).
pub fn parse(text: &str, errors: &mut Vec<ParseError>, options: ParseOptions) -> Option<Value> {
    let mut builder = ValueBuilder { stack: vec![(Value::Array(Vec::new()), None)], errors };
    visit(text, &mut builder, options);
    let (Value::Array(mut root), _) = builder.stack.swap_remove(0) else { unreachable!() };
    (!root.is_empty()).then(|| root.swap_remove(0))
}

/// Builds the value as upstream's `parse` does: containers in the order they open, each with the property it is under.
struct ValueBuilder<'e> {
    stack: Vec<(Value, Option<String>)>,
    errors: &'e mut Vec<ParseError>,
}

impl ValueBuilder<'_> {
    fn add(&mut self, value: Value) {
        let (parent, property) = self.stack.last_mut().unwrap();
        match parent {
            Value::Array(items) => items.push(value),
            Value::Object(map) => {
                if let Some(property) = property {
                    map.insert(property.clone(), value);
                }
            }
            _ => unreachable!(),
        }
    }

    fn close(&mut self) {
        // A container left open by a broken document closes with its parent.
        if self.stack.len() > 1 {
            let (value, _) = self.stack.pop().unwrap();
            self.add(value);
        }
    }
}

impl Visitor for ValueBuilder<'_> {
    fn on_object_begin(&mut self, _: usize, _: usize) {
        self.stack.push((Value::Object(Map::new()), None));
    }
    fn on_object_property(&mut self, name: &str, _: usize, _: usize) {
        self.stack.last_mut().unwrap().1 = Some(name.to_owned());
    }
    fn on_object_end(&mut self, _: usize, _: usize) {
        self.close();
    }
    fn on_array_begin(&mut self, _: usize, _: usize) {
        self.stack.push((Value::Array(Vec::new()), None));
    }
    fn on_array_end(&mut self, _: usize, _: usize) {
        self.close();
    }
    fn on_literal_value(&mut self, value: Value, _: usize, _: usize) {
        self.add(value);
    }
    fn on_error(&mut self, error: ParseErrorCode, offset: usize, length: usize) {
        self.errors.push(ParseError { error, offset, length });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeType {
    Object,
    Array,
    Property,
    String,
    Number,
    Boolean,
    Null,
}

/// One node of `parseTree`; `children` and `parent` index into `Tree::nodes`.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeType,
    pub offset: usize,
    pub length: usize,
    pub value: Value,
    pub colon_offset: Option<usize>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Index(usize),
}

impl Tree {
    pub fn node(&self, index: usize) -> &Node {
        &self.nodes[index]
    }

    /// `findNodeAtLocation(root, path)`.
    pub fn find(&self, start: Option<usize>, path: &[Segment]) -> Option<usize> {
        let mut node = start?;
        for segment in path {
            let current = &self.nodes[node];
            node = match segment {
                Segment::Key(key) => {
                    if current.kind != NodeType::Object {
                        return None;
                    }
                    let property = current.children.iter().map(|&p| &self.nodes[p]).find(|p| p.children.first().is_some_and(|&name| self.nodes[name].value == *key))?;
                    *property.children.get(1)?
                }
                Segment::Index(index) => {
                    if current.kind != NodeType::Array {
                        return None;
                    }
                    *current.children.get(*index)?
                }
            };
        }
        Some(node)
    }
}

/// `parseTree(text, errors, options)`.
pub fn parse_tree(text: &str, errors: &mut Vec<ParseError>, options: ParseOptions) -> Tree {
    let artificial = Node { kind: NodeType::Array, offset: 0, length: 0, value: Value::Null, colon_offset: None, parent: None, children: Vec::new() };
    let mut builder = TreeBuilder { nodes: vec![artificial], current: 0, errors };
    visit(text, &mut builder, options);
    let root = builder.nodes[0].children.first().copied();
    if let Some(root) = root {
        builder.nodes[root].parent = None;
    }
    Tree { nodes: builder.nodes, root }
}

struct TreeBuilder<'e> {
    nodes: Vec<Node>,
    current: usize,
    errors: &'e mut Vec<ParseError>,
}

impl TreeBuilder<'_> {
    fn add(&mut self, kind: NodeType, offset: usize, length: usize, value: Value) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node { kind, offset, length, value, colon_offset: None, parent: Some(self.current), children: Vec::new() });
        self.nodes[self.current].children.push(index);
        index
    }

    fn ensure_property_complete(&mut self, end: usize) {
        if self.nodes[self.current].kind == NodeType::Property {
            let node = &mut self.nodes[self.current];
            node.length = end - node.offset;
            self.current = node.parent.unwrap();
        }
    }

    fn end_container(&mut self, offset: usize, length: usize) {
        let node = &mut self.nodes[self.current];
        node.length = offset + length - node.offset;
        self.current = node.parent.unwrap();
        self.ensure_property_complete(offset + length);
    }
}

impl Visitor for TreeBuilder<'_> {
    fn on_object_begin(&mut self, offset: usize, _: usize) {
        self.current = self.add(NodeType::Object, offset, 0, Value::Null);
    }
    fn on_object_property(&mut self, name: &str, offset: usize, length: usize) {
        self.current = self.add(NodeType::Property, offset, 0, Value::Null);
        self.add(NodeType::String, offset, length, Value::String(name.to_owned()));
    }
    fn on_object_end(&mut self, offset: usize, length: usize) {
        self.end_container(offset, length);
    }
    fn on_array_begin(&mut self, offset: usize, _: usize) {
        self.current = self.add(NodeType::Array, offset, 0, Value::Null);
    }
    fn on_array_end(&mut self, offset: usize, length: usize) {
        self.end_container(offset, length);
    }
    fn on_literal_value(&mut self, value: Value, offset: usize, length: usize) {
        let kind = match &value {
            Value::Bool(_) => NodeType::Boolean,
            Value::Number(_) => NodeType::Number,
            Value::String(_) => NodeType::String,
            _ => NodeType::Null,
        };
        self.add(kind, offset, length, value);
        self.ensure_property_complete(offset + length);
    }
    fn on_separator(&mut self, separator: char, offset: usize, _: usize) {
        if self.nodes[self.current].kind == NodeType::Property {
            if separator == ':' {
                self.nodes[self.current].colon_offset = Some(offset);
            } else if separator == ',' {
                self.ensure_property_complete(offset);
            }
        }
    }
    fn on_error(&mut self, error: ParseErrorCode, offset: usize, length: usize) {
        self.errors.push(ParseError { error, offset, length });
    }
}

/// base/test/common/json.test.ts.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use SyntaxKind::*;

    fn assert_kinds(text: &str, kinds: &[SyntaxKind]) {
        let mut scanner = Scanner::new(text, false);
        let mut actual = Vec::new();
        loop {
            let kind = scanner.scan();
            if kind == Eof {
                break;
            }
            actual.push(kind);
        }
        assert_eq!(actual, kinds, "{text:?}");
    }

    fn assert_scan_error(text: &str, kind: SyntaxKind, error: ScanError) {
        let mut scanner = Scanner::new(text, false);
        scanner.scan();
        assert_eq!((scanner.token(), scanner.token_error()), (kind, error));
    }

    fn options(allow_trailing_comma: bool, disallow_comments: bool) -> ParseOptions {
        ParseOptions { allow_trailing_comma, disallow_comments, ..ParseOptions::default() }
    }

    fn assert_valid(input: &str, expected: Value, options: ParseOptions) {
        let mut errors = Vec::new();
        let actual = parse(input, &mut errors, options);
        assert_eq!(errors, [], "{input:?}");
        assert_eq!(actual, Some(expected), "{input:?}");
    }

    fn assert_invalid(input: &str, expected: Value, options: ParseOptions) {
        let mut errors = Vec::new();
        let actual = parse(input, &mut errors, options);
        assert!(!errors.is_empty(), "{input:?}");
        assert_eq!(actual, Some(expected), "{input:?}");
    }

    /// The tree as upstream's `Node` objects, checking each child's parent on the way.
    fn node_json(tree: &Tree, index: usize) -> Value {
        let node = tree.node(index);
        let kind = match node.kind {
            NodeType::Object => "object",
            NodeType::Array => "array",
            NodeType::Property => "property",
            NodeType::String => "string",
            NodeType::Number => "number",
            NodeType::Boolean => "boolean",
            NodeType::Null => "null",
        };
        let mut json = json!({ "type": kind, "offset": node.offset, "length": node.length });
        if let Some(colon) = node.colon_offset {
            json["colonOffset"] = json!(colon);
        }
        match node.kind {
            NodeType::Object | NodeType::Array | NodeType::Property => {
                for &child in &node.children {
                    assert_eq!(tree.node(child).parent, Some(index));
                }
                json["children"] = node.children.iter().map(|&child| node_json(tree, child)).collect();
            }
            _ => json["value"] = node.value.clone(),
        }
        json
    }

    fn assert_tree(input: &str, expected: Value, expected_errors: &[ParseErrorCode], options: ParseOptions) {
        let mut errors = Vec::new();
        let tree = parse_tree(input, &mut errors, options);
        assert_eq!(errors.iter().map(|error| error.error).collect::<Vec<_>>(), expected_errors);
        assert_eq!(node_json(&tree, tree.root.unwrap()), expected);
    }

    #[test]
    fn tokens() {
        assert_kinds("{", &[OpenBraceToken]);
        assert_kinds("}", &[CloseBraceToken]);
        assert_kinds("[", &[OpenBracketToken]);
        assert_kinds("]", &[CloseBracketToken]);
        assert_kinds(":", &[ColonToken]);
        assert_kinds(",", &[CommaToken]);
    }

    #[test]
    fn comments() {
        assert_kinds("// this is a comment", &[LineCommentTrivia]);
        assert_kinds("// this is a comment\n", &[LineCommentTrivia, LineBreakTrivia]);
        assert_kinds("/* this is a comment*/", &[BlockCommentTrivia]);
        assert_kinds("/* this is a \r\ncomment*/", &[BlockCommentTrivia]);
        assert_kinds("/* this is a \ncomment*/", &[BlockCommentTrivia]);
        assert_kinds("/* this is a", &[BlockCommentTrivia]);
        assert_kinds("/* this is a \ncomment", &[BlockCommentTrivia]);
        assert_kinds("/ ttt", &[Unknown, Trivia, Unknown]);
    }

    #[test]
    fn strings() {
        for text in ["\"test\"", "\"\\\"\"", "\"\\/\"", "\"\\b\"", "\"\\f\"", "\"\\n\"", "\"\\r\"", "\"\\t\"", "\"\\v\"", "\"\u{88ff}\"", "\"\u{200b}\u{2028}\"", "\"test"] {
            assert_kinds(text, &[StringLiteral]);
        }
        assert_kinds("\"test\n\"", &[StringLiteral, LineBreakTrivia, StringLiteral]);
        assert_scan_error("\"\t\"", StringLiteral, ScanError::InvalidCharacter);
        assert_scan_error("\"\t \"", StringLiteral, ScanError::InvalidCharacter);
    }

    #[test]
    fn numbers() {
        for text in ["0", "0.1", "-0.1", "-1", "1", "123456789", "10", "90", "90E+123", "90e+123", "90e-123", "90E-123", "90E123", "90e123"] {
            assert_kinds(text, &[NumericLiteral]);
        }
        assert_kinds("01", &[NumericLiteral, NumericLiteral]);
        assert_kinds("-01", &[NumericLiteral, NumericLiteral]);
        assert_kinds("-", &[Unknown]);
        assert_kinds(".0", &[Unknown]);
    }

    #[test]
    fn keywords() {
        assert_kinds("true", &[TrueKeyword]);
        assert_kinds("false", &[FalseKeyword]);
        assert_kinds("null", &[NullKeyword]);
        assert_kinds("true false null", &[TrueKeyword, Trivia, FalseKeyword, Trivia, NullKeyword]);
        assert_kinds("nulllll", &[Unknown]);
        assert_kinds("True", &[Unknown]);
        assert_kinds("foo-bar", &[Unknown]);
        assert_kinds("foo bar", &[Unknown, Trivia, Unknown]);
    }

    #[test]
    fn trivia() {
        assert_kinds(" ", &[Trivia]);
        assert_kinds("  \t  ", &[Trivia]);
        assert_kinds("  \t  \n  \t  ", &[Trivia, LineBreakTrivia, Trivia]);
        assert_kinds("\r\n", &[LineBreakTrivia]);
        assert_kinds("\r", &[LineBreakTrivia]);
        assert_kinds("\n", &[LineBreakTrivia]);
        assert_kinds("\n\r", &[LineBreakTrivia, LineBreakTrivia]);
        assert_kinds("\n   \n", &[LineBreakTrivia, Trivia, LineBreakTrivia]);
    }

    #[test]
    fn parse_literals() {
        let default = ParseOptions::default();
        assert_valid("true", json!(true), default);
        assert_valid("false", json!(false), default);
        assert_valid("null", json!(null), default);
        assert_valid("\"foo\"", json!("foo"), default);
        assert_valid(r#""\"-\\-\/-\b-\f-\n-\r-\t""#, json!("\"-\\-/-\u{8}-\u{c}-\n-\r-\t"), default);
        assert_valid(r#""\u00DC""#, json!("Ü"), default);
        assert_valid("9", json!(9), default);
        assert_valid("-9", json!(-9), default);
        assert_valid("0.129", json!(0.129), default);
        assert_valid("23e3", json!(23e3), default);
        assert_valid("1.2E+3", json!(1.2E+3), default);
        assert_valid("1.2E-3", json!(1.2E-3), default);
        assert_valid("1.2E-3 // comment", json!(1.2E-3), default);
    }

    #[test]
    fn parse_objects() {
        let default = ParseOptions::default();
        assert_valid("{}", json!({}), default);
        assert_valid(r#"{ "foo": true }"#, json!({ "foo": true }), default);
        assert_valid(r#"{ "bar": 8, "xoo": "foo" }"#, json!({ "bar": 8, "xoo": "foo" }), default);
        assert_valid(r#"{ "hello": [], "world": {} }"#, json!({ "hello": [], "world": {} }), default);
        assert_valid(r#"{ "a": false, "b": true, "c": [ 7.4 ] }"#, json!({ "a": false, "b": true, "c": [7.4] }), default);
        assert_valid(
            r#"{ "lineComment": "//", "blockComment": ["/*", "*/"], "brackets": [ ["{", "}"], ["[", "]"], ["(", ")"] ] }"#,
            json!({ "lineComment": "//", "blockComment": ["/*", "*/"], "brackets": [["{", "}"], ["[", "]"], ["(", ")"]] }),
            default,
        );
        assert_valid(r#"{ "hello": { "again": { "inside": 5 }, "world": 1 }}"#, json!({ "hello": { "again": { "inside": 5 }, "world": 1 } }), default);
        assert_valid(r#"{ "foo": /*hello*/true }"#, json!({ "foo": true }), default);
        assert_valid(r#"{ "__proto__": { "polluted": true } }"#, json!({ "__proto__": { "polluted": true } }), default);
    }

    #[test]
    fn parse_arrays() {
        let default = ParseOptions::default();
        assert_valid("[]", json!([]), default);
        assert_valid("[ [],  [ [] ]]", json!([[], [[]]]), default);
        assert_valid("[ 1, 2, 3 ]", json!([1, 2, 3]), default);
        assert_valid(r#"[ { "a": null } ]"#, json!([{ "a": null }]), default);
    }

    #[test]
    fn parse_objects_with_errors() {
        let default = ParseOptions::default();
        assert_invalid("{,}", json!({}), default);
        assert_invalid(r#"{ "foo": true, }"#, json!({ "foo": true }), options(false, false));
        assert_invalid(r#"{ "bar": 8 "xoo": "foo" }"#, json!({ "bar": 8, "xoo": "foo" }), default);
        assert_invalid(r#"{ ,"bar": 8 }"#, json!({ "bar": 8 }), default);
        assert_invalid(r#"{ ,"bar": 8, "foo" }"#, json!({ "bar": 8 }), default);
        assert_invalid(r#"{ "bar": 8, "foo": }"#, json!({ "bar": 8 }), default);
        assert_invalid(r#"{ 8, "foo": 9 }"#, json!({ "foo": 9 }), default);
    }

    #[test]
    fn parse_arrays_with_errors() {
        let default = ParseOptions::default();
        assert_invalid("[,]", json!([]), default);
        assert_invalid("[ 1, 2, ]", json!([1, 2]), options(false, false));
        assert_invalid("[ 1 2, 3 ]", json!([1, 2, 3]), default);
        assert_invalid("[ ,1, 2, 3 ]", json!([1, 2, 3]), default);
        assert_invalid("[ ,1, 2, 3, ]", json!([1, 2, 3]), options(false, false));
    }

    #[test]
    fn parse_disallow_comments() {
        let no_comments = options(true, true);
        assert_valid(r#"[ 1, 2, null, "foo" ]"#, json!([1, 2, null, "foo"]), no_comments);
        assert_valid(r#"{ "hello": [], "world": {} }"#, json!({ "hello": [], "world": {} }), no_comments);
        assert_invalid(r#"{ "foo": /*comment*/ true }"#, json!({ "foo": true }), no_comments);
    }

    #[test]
    fn parse_trailing_comma() {
        let allow = options(true, false);
        assert_valid(r#"{ "hello": [], }"#, json!({ "hello": [] }), ParseOptions::default());
        assert_valid(r#"{ "hello": [], }"#, json!({ "hello": [] }), allow);
        assert_valid(r#"{ "hello": [] }"#, json!({ "hello": [] }), allow);
        assert_valid(r#"{ "hello": [], "world": {}, }"#, json!({ "hello": [], "world": {} }), allow);
        assert_valid(r#"{ "hello": [], "world": {} }"#, json!({ "hello": [], "world": {} }), allow);
        assert_valid(r#"{ "hello": [1,] }"#, json!({ "hello": [1] }), allow);
        let disallow = options(false, false);
        assert_invalid(r#"{ "hello": [], }"#, json!({ "hello": [] }), disallow);
        assert_invalid(r#"{ "hello": [], "world": {}, }"#, json!({ "hello": [], "world": {} }), disallow);
    }

    #[test]
    fn tree_literals() {
        let default = ParseOptions::default();
        assert_tree("true", json!({ "type": "boolean", "offset": 0, "length": 4, "value": true }), &[], default);
        assert_tree("false", json!({ "type": "boolean", "offset": 0, "length": 5, "value": false }), &[], default);
        assert_tree("null", json!({ "type": "null", "offset": 0, "length": 4, "value": null }), &[], default);
        assert_tree("23", json!({ "type": "number", "offset": 0, "length": 2, "value": 23 }), &[], default);
        assert_tree("-1.93e-19", json!({ "type": "number", "offset": 0, "length": 9, "value": -1.93e-19 }), &[], default);
        assert_tree("\"hello\"", json!({ "type": "string", "offset": 0, "length": 7, "value": "hello" }), &[], default);
    }

    #[test]
    fn tree_arrays() {
        let default = ParseOptions::default();
        assert_tree("[]", json!({ "type": "array", "offset": 0, "length": 2, "children": [] }), &[], default);
        assert_tree("[ 1 ]", json!({ "type": "array", "offset": 0, "length": 5, "children": [{ "type": "number", "offset": 2, "length": 1, "value": 1 }] }), &[], default);
        assert_tree(
            "[ 1,\"x\"]",
            json!({ "type": "array", "offset": 0, "length": 8, "children": [
                { "type": "number", "offset": 2, "length": 1, "value": 1 },
                { "type": "string", "offset": 4, "length": 3, "value": "x" }
            ] }),
            &[],
            default,
        );
        assert_tree("[[]]", json!({ "type": "array", "offset": 0, "length": 4, "children": [{ "type": "array", "offset": 1, "length": 2, "children": [] }] }), &[], default);
    }

    #[test]
    fn tree_objects() {
        let default = ParseOptions::default();
        assert_tree("{ }", json!({ "type": "object", "offset": 0, "length": 3, "children": [] }), &[], default);
        assert_tree(
            r#"{ "val": 1 }"#,
            json!({ "type": "object", "offset": 0, "length": 12, "children": [
                { "type": "property", "offset": 2, "length": 8, "colonOffset": 7, "children": [
                    { "type": "string", "offset": 2, "length": 5, "value": "val" },
                    { "type": "number", "offset": 9, "length": 1, "value": 1 }
                ] }
            ] }),
            &[],
            default,
        );
        assert_tree(
            r#"{"id": "$", "v": [ null, null] }"#,
            json!({ "type": "object", "offset": 0, "length": 32, "children": [
                { "type": "property", "offset": 1, "length": 9, "colonOffset": 5, "children": [
                    { "type": "string", "offset": 1, "length": 4, "value": "id" },
                    { "type": "string", "offset": 7, "length": 3, "value": "$" }
                ] },
                { "type": "property", "offset": 12, "length": 18, "colonOffset": 15, "children": [
                    { "type": "string", "offset": 12, "length": 3, "value": "v" },
                    { "type": "array", "offset": 17, "length": 13, "children": [
                        { "type": "null", "offset": 19, "length": 4, "value": null },
                        { "type": "null", "offset": 25, "length": 4, "value": null }
                    ] }
                ] }
            ] }),
            &[],
            default,
        );
        assert_tree(
            r#"{  "id": { "foo": { } } , }"#,
            json!({ "type": "object", "offset": 0, "length": 27, "children": [
                { "type": "property", "offset": 3, "length": 20, "colonOffset": 7, "children": [
                    { "type": "string", "offset": 3, "length": 4, "value": "id" },
                    { "type": "object", "offset": 9, "length": 14, "children": [
                        { "type": "property", "offset": 11, "length": 10, "colonOffset": 16, "children": [
                            { "type": "string", "offset": 11, "length": 5, "value": "foo" },
                            { "type": "object", "offset": 18, "length": 3, "children": [] }
                        ] }
                    ] }
                ] }
            ] }),
            &[ParseErrorCode::PropertyNameExpected, ParseErrorCode::ValueExpected],
            options(false, false),
        );
    }
}
