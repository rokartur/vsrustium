//! Bracket pair colorization: the bracket tokens of a language (bracketPairsTree/brackets.ts), the
//! tokenizer over the TextMate tokens (tokenizer.ts), the pair parser (parser.ts) and the nesting levels
//! `collectBrackets` hands to the decoration provider (bracketPairsTree.ts).

use std::collections::HashMap;
use std::ops::Range;

use serde_json::Value;
use textmate::{StandardTokenType, Style};

/// The brackets of one language: `LanguageBracketsConfiguration` turned into `BracketTokens`.
pub struct LanguageBrackets {
    tokens: HashMap<String, BracketToken>,
    regex: Option<onig::Regex>,
}

struct BracketToken {
    opening: bool,
    /// `languageId:::text` of the bracket, or of the first opening bracket a closing one closes.
    id: String,
    /// The opening brackets this one opens or closes.
    ids: Vec<String>,
    /// The opening brackets a closing one closes as a colorized pair.
    colorized_openings: Vec<String>,
}

impl LanguageBrackets {
    /// From a language configuration: `brackets`, and `colorizedBracketPairs`, which defaults to the
    /// brackets except `<>`. Every colorized pair is a bracket pair too.
    pub fn new(language: &str, config: &Value) -> Self {
        let brackets = character_pairs(&config["brackets"]).unwrap_or_default();
        let colorized = character_pairs(&config["colorizedBracketPairs"])
            .unwrap_or_else(|| brackets.iter().filter(|(open, close)| !(open == "<" && close == ">")).cloned().collect());
        let id = |text: &str| format!("{language}:::{text}");
        // Insertion ordered, as the CachedFunction maps of LanguageBracketsConfiguration.
        let mut openings: Vec<&str> = Vec::new();
        let mut closings: Vec<(&str, Vec<String>, Vec<String>)> = Vec::new();
        for (index, (open, close)) in brackets.iter().chain(&colorized).enumerate() {
            if !openings.contains(&open.as_str()) {
                openings.push(open);
            }
            let position = match closings.iter().position(|(text, ..)| text == close) {
                Some(position) => position,
                None => {
                    closings.push((close, Vec::new(), Vec::new()));
                    closings.len() - 1
                }
            };
            let (_, opened, colorized_opened) = &mut closings[position];
            if !opened.contains(&id(open)) {
                opened.push(id(open));
            }
            if index >= brackets.len() && !colorized_opened.contains(&id(open)) {
                colorized_opened.push(id(open));
            }
        }
        let mut tokens = HashMap::new();
        for open in openings {
            tokens.insert(open.to_string(), BracketToken { opening: true, id: id(open), ids: vec![id(open)], colorized_openings: Vec::new() });
        }
        for (close, opened, colorized_openings) in closings {
            tokens.insert(close.to_string(), BracketToken { opening: false, id: opened[0].clone(), ids: opened, colorized_openings });
        }
        let mut texts: Vec<&String> = tokens.keys().collect();
        texts.sort();
        texts.reverse();
        let pattern = texts.iter().map(|text| bracket_pattern(text)).collect::<Vec<_>>().join("|");
        let regex = (!texts.is_empty()).then(|| {
            onig::Regex::with_options(&pattern, onig::RegexOptions::REGEX_OPTION_IGNORECASE, onig::Syntax::default()).expect("escaped brackets compile")
        });
        LanguageBrackets { tokens, regex }
    }

    /// `BracketTokens.getToken`: the map holds the configured texts, the lookup is lower case.
    fn token(&self, text: &str) -> Option<&BracketToken> {
        self.tokens.get(&text.to_lowercase())
    }

    /// `ClosingBracketKind.closes`: `close` is a closing bracket of the opening bracket `open`.
    pub fn closes(&self, close: &str, open: &str) -> bool {
        let (Some(close), Some(open)) = (self.token(close), self.token(open)) else { return false };
        !close.opening && close.ids.contains(&open.id)
    }

    /// `text.replace(getBracketRegExp({ global: true }), '')`: the text without any bracket.
    pub fn strip(&self, text: &str) -> String {
        match &self.regex {
            Some(regex) => regex.replace_all(text, ""),
            None => text.to_string(),
        }
    }
}

/// `_extractValidBrackets`: the `[open, close]` string pairs of an array, empty strings dropped
/// (`filterValidBrackets`); `None` when the key is not an array.
pub(crate) fn character_pairs(value: &Value) -> Option<Vec<(String, String)>> {
    let pairs = value.as_array()?.iter().filter_map(|pair| match pair.as_array()?.as_slice() {
        [Value::String(open), Value::String(close)] if !open.is_empty() && !close.is_empty() => Some((open.clone(), close.clone())),
        _ => None,
    });
    Some(pairs.collect())
}

/// `prepareBracketForRegExp`: escaped, with word boundaries where the bracket starts or ends with a
/// word character or space (#132162, #150440).
pub(crate) fn bracket_pattern(text: &str) -> String {
    let mut escaped = escape_regex(text);
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ');
    if word(text.chars().next()) {
        escaped.insert_str(0, "\\b");
    }
    if word(text.chars().last()) {
        escaped.push_str("\\b");
    }
    escaped
}

/// `escapeRegExpCharacters`.
pub(crate) fn escape_regex(text: &str) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        if "\\{}*+?|^$.[]()".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// A token of a line as the bracket tokenizer reads its metadata.
#[derive(Clone, Copy)]
pub struct Span<'a> {
    pub end: usize,
    pub language: &'a str,
    pub token_type: StandardTokenType,
    pub balanced_brackets: bool,
    pub style: Style,
}

impl Span<'_> {
    fn same_metadata(&self, other: &Span) -> bool {
        (self.language, self.token_type, self.balanced_brackets, self.style) == (other.language, other.token_type, other.balanced_brackets, other.style)
    }
}

/// A bracket to color: `level` indexes the bracket colors, `invalid` takes the unexpected bracket color.
#[derive(Clone, Debug, PartialEq)]
pub struct Bracket {
    pub range: Range<usize>,
    pub level: isize,
    pub invalid: bool,
}

/// A bracket's line and byte range in it.
pub type BracketRange = (usize, Range<usize>);

/// A pair of brackets; `close` is `None` for an opening bracket nothing closes.
pub struct Pair {
    pub open: BracketRange,
    pub close: Option<BracketRange>,
}

/// A bracket of a pair (`IFoundBracket`): `key` is the `languageId:::text` of the bracket, or of the first
/// opening bracket a closing one closes.
pub struct PairBracket {
    pub range: BracketRange,
    pub opening: bool,
    pub key: String,
}

/// The brackets of a whole document: the colorized brackets of each line
/// (`getBracketsInRange(range, true)`), every pair, outer pairs first (`getBracketPairsInRange`), and
/// every bracket of a pair in document order (`getFirstBracketAfter`/`Before` skip unexpected closings).
#[derive(Default)]
pub struct DocumentBrackets {
    pub lines: Vec<Vec<Bracket>>,
    pub pairs: Vec<Pair>,
    pub brackets: Vec<PairBracket>,
}

impl DocumentBrackets {
    pub fn new(lines: &[(&str, Vec<Span>)], languages: &HashMap<String, LanguageBrackets>) -> Self {
        let found = scan(lines, languages);
        let mut parser = Parser { found: &found, next: 0 };
        let mut nodes = Vec::new();
        while parser.next < found.len() {
            nodes.extend(parser.parse_child(&[], 1));
        }
        let mut result = DocumentBrackets { lines: vec![Vec::new(); lines.len()], pairs: Vec::new(), brackets: Vec::new() };
        collect(&nodes, &found, 0, &mut result.lines);
        collect_pairs(&nodes, &found, &mut result.pairs);
        collect_pair_brackets(&nodes, &found, &mut result.brackets);
        result
    }

    /// `matchBracket`: the closed pair with a bracket that touches the position (line, byte), the
    /// bracket starting last winning.
    pub fn match_bracket(&self, line: usize, offset: usize) -> Option<[&BracketRange; 2]> {
        let touches = |(bracket_line, range): &BracketRange| *bracket_line == line && range.start <= offset && offset <= range.end;
        let mut best: Option<(&BracketRange, [&BracketRange; 2])> = None;
        for pair in &self.pairs {
            let Some(close) = &pair.close else { continue };
            let bracket = if touches(&pair.open) {
                &pair.open
            } else if touches(close) {
                close
            } else {
                continue;
            };
            if best.is_none_or(|(best, _)| (bracket.0, bracket.1.start) >= (best.0, best.1.start)) {
                best = Some((bracket, [&pair.open, close]));
            }
        }
        best.map(|(_, pair)| pair)
    }

    /// `findNextBracket` (`getFirstBracketAfter`): the first bracket of a pair that ends after the position (line, byte).
    pub fn next_bracket(&self, line: usize, offset: usize) -> Option<&BracketRange> {
        self.brackets.iter().map(|bracket| &bracket.range).find(|(bracket_line, range)| (*bracket_line, range.end) > (line, offset))
    }

    /// `findEnclosingBrackets`: the innermost closed pair strictly around the position (line, byte).
    pub fn enclosing_brackets(&self, line: usize, offset: usize) -> Option<[&BracketRange; 2]> {
        self.pairs.iter().rev().find_map(|pair| {
            let close = pair.close.as_ref()?;
            let after_open = (line, offset) > (pair.open.0, pair.open.1.start);
            let before_close = (line, offset) < (close.0, close.1.end);
            (after_open && before_close).then_some([&pair.open, close])
        })
    }
}

struct Found<'a> {
    line: usize,
    range: Range<usize>,
    token: &'a BracketToken,
}

/// `NonPeekableTextBufferTokenizer.read`: brackets only in `Other` tokens that may hold balanced
/// brackets, with neighbor tokens of equal metadata read as one.
fn scan<'a>(lines: &[(&str, Vec<Span>)], languages: &'a HashMap<String, LanguageBrackets>) -> Vec<Found<'a>> {
    let mut found = Vec::new();
    for (line, (text, spans)) in lines.iter().enumerate() {
        let (mut position, mut index) = (0, 0);
        while index < spans.len() {
            let span = spans[index];
            while index + 1 < spans.len() && spans[index + 1].same_metadata(&span) {
                index += 1;
            }
            let end = spans[index].end.min(text.len());
            let mut bracket = None;
            if span.token_type == StandardTokenType::Other && span.balanced_brackets && position < end
                && let Some(brackets) = languages.get(span.language)
                && let Some(regex) = &brackets.regex
                && let Some((start, match_end)) = regex.find(&text[position..end])
            {
                bracket = brackets.token(&text[position + start..position + match_end]).map(|token| (position + start..position + match_end, token));
            }
            match bracket {
                // A token can hold more brackets, so read on from the bracket's end.
                Some((range, token)) => {
                    position = range.end;
                    found.push(Found { line, range, token });
                }
                None => {
                    index += 1;
                    position = end;
                }
            }
        }
    }
    found
}

enum Node {
    Pair { open: usize, children: Vec<Node>, close: Option<usize> },
    Unexpected(usize),
}

/// `Parser` (parser.ts) without the incremental reuse: an opening bracket starts a pair that a closing
/// bracket of the same set ends; a closing bracket of an outer pair ends the inner ones unclosed.
struct Parser<'a> {
    found: &'a [Found<'a>],
    next: usize,
}

impl Parser<'_> {
    fn parse_list(&mut self, opened: &[&str], level: usize) -> Vec<Node> {
        let mut items = Vec::new();
        while let Some(next) = self.found.get(self.next) {
            if !next.token.opening && intersects(&next.token.ids, opened) {
                break;
            }
            items.extend(self.parse_child(opened, level + 1));
        }
        items
    }

    fn parse_child(&mut self, opened: &[&str], level: usize) -> Option<Node> {
        let index = self.next;
        self.next += 1;
        let token = self.found[index].token;
        if !token.opening {
            return Some(Node::Unexpected(index));
        }
        // Deeper openings read as text.
        if level > 300 {
            return None;
        }
        let mut set = opened.to_vec();
        for id in &token.ids {
            if !set.contains(&id.as_str()) {
                set.push(id);
            }
        }
        let children = self.parse_list(&set, level + 1);
        let mut close = None;
        if let Some(next) = self.found.get(self.next)
            && !next.token.opening
            && (next.token.id == token.id || next.token.ids.iter().any(|id| token.ids.contains(id)))
        {
            close = Some(self.next);
            self.next += 1;
        }
        Some(Node::Pair { open: index, children, close })
    }
}

fn collect_pair_brackets(nodes: &[Node], found: &[Found], result: &mut Vec<PairBracket>) {
    for node in nodes {
        let Node::Pair { open, children, close } = node else { continue };
        let bracket = |index: usize| PairBracket { range: (found[index].line, found[index].range.clone()), opening: found[index].token.opening, key: found[index].token.id.clone() };
        result.push(bracket(*open));
        collect_pair_brackets(children, found, result);
        if let Some(close) = close {
            result.push(bracket(*close));
        }
    }
}

fn intersects(ids: &[String], opened: &[&str]) -> bool {
    ids.iter().any(|id| opened.contains(&id.as_str()))
}

/// `collectBrackets` with `onlyColorizedBrackets`: a pair that is not colorized (`<>` by default) adds
/// no level and shows no brackets; an unclosed pair's opening bracket is invalid.
fn collect(nodes: &[Node], found: &[Found], level: isize, result: &mut [Vec<Bracket>]) {
    if level > 200 {
        return;
    }
    for node in nodes {
        match node {
            Node::Unexpected(index) => push(result, &found[*index], level - 1, true),
            Node::Pair { open, children, close } => {
                let colorize = close.is_none_or(|close| found[close].token.colorized_openings.contains(&found[*open].token.id));
                let child_level = if colorize { level + 1 } else { level };
                let visible = colorize && child_level <= 200;
                if visible {
                    push(result, &found[*open], child_level - 1, close.is_none());
                }
                collect(children, found, child_level, result);
                if let Some(close) = close
                    && visible
                {
                    push(result, &found[*close], child_level - 1, false);
                }
            }
        }
    }
}

/// `collectBracketPairs`: every pair, colorized or not, before the pairs inside it.
fn collect_pairs(nodes: &[Node], found: &[Found], pairs: &mut Vec<Pair>) {
    for node in nodes {
        if let Node::Pair { open, children, close } = node {
            let range = |index: usize| (found[index].line, found[index].range.clone());
            pairs.push(Pair { open: range(*open), close: close.map(range) });
            collect_pairs(children, found, pairs);
        }
    }
}

fn push(result: &mut [Vec<Bracket>], found: &Found, level: isize, invalid: bool) {
    result[found.line].push(Bracket { range: found.range.clone(), level, invalid });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// One `Other` token per line that may hold brackets, as `TokenizedDocument` in the upstream tests.
    fn brackets_of(text: &str, config: Value) -> Vec<(isize, Range<usize>, bool)> {
        let languages = HashMap::from([("test".to_string(), LanguageBrackets::new("test", &config))]);
        let style = Style { font_style: 0, foreground: 1, background: 2 };
        let span = Span { end: text.len(), language: "test", token_type: StandardTokenType::Other, balanced_brackets: true, style };
        DocumentBrackets::new(&[(text, vec![span])], &languages).lines[0].iter().map(|b| (b.level, b.range.clone(), b.invalid)).collect()
    }

    fn colorized_config() -> Value {
        json!({ "brackets": [["<", ">"]], "colorizedBracketPairs": [["{", "}"], ["[", "]"], ["(", ")"]] })
    }

    /// brackets.test.ts "Basic".
    #[test]
    fn closing_brackets_close_every_opening_they_pair_with() {
        let config = json!({ "brackets": [
            ["{", "}"], ["[", "]"], ["(", ")"],
            ["begin", "end"], ["case", "endcase"], ["casez", "endcase"],
            ["\\left(", "\\right)"], ["\\left(", "\\right."], ["\\left.", "\\right)"],
            ["\\left[", "\\right]"], ["\\left[", "\\right."], ["\\left.", "\\right]"]
        ] });
        let brackets = LanguageBrackets::new("testMode1", &config);
        let token = |text: &str| {
            let token = brackets.token(text).unwrap();
            let strip = |id: &str| id.trim_start_matches("testMode1:::").to_string();
            (token.opening, strip(&token.id), token.ids.iter().map(|id| strip(id)).collect::<Vec<_>>())
        };
        assert_eq!(token("begin"), (true, "begin".into(), vec!["begin".into()]));
        assert_eq!(token("\\left."), (true, "\\left.".into(), vec!["\\left.".into()]));
        assert_eq!(token("end"), (false, "begin".into(), vec!["begin".into()]));
        assert_eq!(token("endcase"), (false, "case".into(), vec!["case".into(), "casez".into()]));
        assert_eq!(token("\\right)"), (false, "\\left(".into(), vec!["\\left(".into(), "\\left.".into()]));
        assert_eq!(token("\\right."), (false, "\\left(".into(), vec!["\\left(".into(), "\\left[".into()]));
        assert_eq!(token("\\right]"), (false, "\\left[".into(), vec!["\\left[".into(), "\\left.".into()]));
    }

    /// getBracketPairsInRange.test.ts "getBracketsInRange"; ranges are 0-based byte offsets.
    #[test]
    fn levels_follow_the_nesting() {
        let levels: Vec<isize> = brackets_of(" { [ ( [ [ (  ) ] ] ) ] } { } ", colorized_config()).into_iter().map(|(level, ..)| level).collect();
        assert_eq!(levels, [0, 1, 2, 3, 4, 5, 5, 4, 3, 2, 1, 0, 0, 0]);
    }

    /// "Test Error Brackets".
    #[test]
    fn unclosed_and_unexpected_brackets_are_invalid() {
        assert_eq!(brackets_of(" { () ] ", colorized_config()), [(0, 1..2, true), (1, 3..4, false), (1, 4..5, false), (0, 6..7, true)]);
    }

    /// "colorizedBracketsVSBrackets" with only the colorized brackets.
    #[test]
    fn pairs_that_are_not_colorized_add_no_level() {
        assert_eq!(
            brackets_of(" {} [<()>] <{>} ", colorized_config()),
            [(0, 1..2, false), (0, 2..3, false), (0, 4..5, false), (1, 6..7, false), (1, 7..8, false), (0, 9..10, false), (0, 12..13, true), (-1, 14..15, true)]
        );
    }

    #[test]
    fn matches_the_touched_bracket_else_the_enclosing_pair() {
        let text = "{ (abc), [def] }";
        let languages = HashMap::from([("test".to_string(), LanguageBrackets::new("test", &json!({ "brackets": [["{", "}"], ["[", "]"], ["(", ")"]] })))]);
        let style = Style { font_style: 0, foreground: 1, background: 2 };
        let span = Span { end: text.len(), language: "test", token_type: StandardTokenType::Other, balanced_brackets: true, style };
        let brackets = DocumentBrackets::new(&[(text, vec![span])], &languages);
        let ranges = |pair: Option<[&BracketRange; 2]>| pair.map(|[open, close]| (open.1.clone(), close.1.clone()));
        assert_eq!(ranges(brackets.match_bracket(0, 2)), Some((2..3, 6..7)));
        // Right after ")": a bracket touches the position from either side.
        assert_eq!(ranges(brackets.match_bracket(0, 7)), Some((2..3, 6..7)));
        assert_eq!(ranges(brackets.match_bracket(0, 1)), Some((0..1, 15..16)));
        assert_eq!(ranges(brackets.match_bracket(0, 4)), None);
        assert_eq!(ranges(brackets.enclosing_brackets(0, 4)), Some((2..3, 6..7)));
        assert_eq!(ranges(brackets.enclosing_brackets(0, 8)), Some((0..1, 15..16)));
        assert_eq!(ranges(brackets.enclosing_brackets(0, 0)), None);
        // Between ")(" both brackets touch; the one starting later wins.
        let text = "()()";
        let span = Span { end: text.len(), ..span };
        let brackets = DocumentBrackets::new(&[(text, vec![span])], &languages);
        assert_eq!(ranges(brackets.match_bracket(0, 2)), Some((2..3, 3..4)));
    }

    #[test]
    fn word_brackets_need_word_boundaries() {
        let config = json!({ "brackets": [["begin", "end"]] });
        assert_eq!(brackets_of("begin x beginning end", config), [(0, 0..5, false), (0, 18..21, false)]);
    }
}
