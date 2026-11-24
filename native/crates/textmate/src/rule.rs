//! Rules and their compiled scanners, as vscode-textmate's rule.ts builds them from raw grammars.

use std::cell::RefCell;
use std::rc::Rc;

use onig::{Regex, RegexOptions, Region, SearchOptions, Syntax};

/// Rule ids are indexes into `Rules::rules`; the scanner uses these two for a matched `end` or `while`.
pub const END_RULE_ID: i64 = -1;
pub const WHILE_RULE_ID: i64 = -2;

/// Start and end byte offsets of each group; a group that did not take part is `(usize::MAX, usize::MAX)`,
/// which reads as empty text, as vscode-oniguruma's unsigned -1 offsets do.
pub type Captures = Vec<(usize, usize)>;

pub fn capture_text<'a>(line: &'a str, capture: (usize, usize)) -> &'a str {
    line.get(capture.0..capture.1).unwrap_or("")
}

pub struct Rule {
    name: Option<String>,
    name_is_capturing: bool,
    content_name: Option<String>,
    content_name_is_capturing: bool,
    pub kind: RuleKind,
    /// Some of the rule's patterns were includes that resolved to nothing.
    pub has_missing_patterns: bool,
    pub(crate) cached: RefCell<Option<RegExpSourceList>>,
    pub(crate) cached_while: RefCell<Option<RegExpSourceList>>,
}

pub enum RuleKind {
    Capture { retokenize_with: Option<usize> },
    Match { regex: RegExpSource, captures: Vec<Option<usize>> },
    IncludeOnly { patterns: Vec<usize> },
    BeginEnd { begin: RegExpSource, begin_captures: Vec<Option<usize>>, end: RegExpSource, end_captures: Vec<Option<usize>>, apply_end_pattern_last: bool, patterns: Vec<usize> },
    BeginWhile { begin: RegExpSource, begin_captures: Vec<Option<usize>>, while_: RegExpSource, while_captures: Vec<Option<usize>>, patterns: Vec<usize> },
}

impl Rule {
    pub fn new(name: Option<String>, content_name: Option<String>, kind: RuleKind) -> Rule {
        Rule {
            name_is_capturing: name.as_deref().is_some_and(has_captures),
            content_name_is_capturing: content_name.as_deref().is_some_and(has_captures),
            name,
            content_name,
            kind,
            has_missing_patterns: false,
            cached: RefCell::default(),
            cached_while: RefCell::default(),
        }
    }

    pub fn name(&self, line: Option<&str>, captures: Option<&Captures>) -> Option<String> {
        match (self.name_is_capturing, line, captures) {
            (true, Some(line), Some(captures)) => Some(replace_captures(self.name.as_deref()?, line, captures)),
            _ => self.name.clone(),
        }
    }

    pub fn content_name(&self, line: &str, captures: &Captures) -> Option<String> {
        if !self.content_name_is_capturing {
            return self.content_name.clone();
        }
        Some(replace_captures(self.content_name.as_deref()?, line, captures))
    }

    /// For the `hasMissingPatterns && patterns.length === 0` check of `_compilePatterns`.
    pub fn patterns(&self) -> Option<&[usize]> {
        match &self.kind {
            RuleKind::IncludeOnly { patterns } | RuleKind::BeginEnd { patterns, .. } | RuleKind::BeginWhile { patterns, .. } => Some(patterns),
            _ => None,
        }
    }
}

/// `$1` or `${1:/downcase}` style references in a name.
fn has_captures(source: &str) -> bool {
    let bytes = source.as_bytes();
    (0..bytes.len()).any(|i| bytes[i] == b'$' && capture_reference(source, i).is_some())
}

/// Parses `$N` or `${N:/downcase|upcase}` at `i`: (capture index, command, length).
fn capture_reference(source: &str, i: usize) -> Option<(usize, Option<&str>, usize)> {
    let rest = &source[i + 1..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        return Some((rest[..digits].parse().ok()?, None, 1 + digits));
    }
    let inner = rest.strip_prefix('{')?;
    let digits = inner.bytes().take_while(u8::is_ascii_digit).count();
    let after = inner[digits..].strip_prefix(":/")?;
    for command in ["downcase", "upcase"] {
        if digits > 0 && after.starts_with(command) && after[command.len()..].starts_with('}') {
            return Some((inner[..digits].parse().ok()?, Some(command), 2 + digits + 2 + command.len() + 1));
        }
    }
    None
}

/// `RegexSource.replaceCaptures`.
fn replace_captures(source: &str, line: &str, captures: &Captures) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < source.len() {
        if source.as_bytes()[i] == b'$'
            && let Some((index, command, length)) = capture_reference(source, i)
        {
            match captures.get(index) {
                Some(&capture) => {
                    let text = capture_text(line, capture).trim_start_matches('.');
                    match command {
                        Some("downcase") => out.push_str(&text.to_lowercase()),
                        Some("upcase") => out.push_str(&text.to_uppercase()),
                        _ => out.push_str(text),
                    }
                }
                None => out.push_str(&source[i..i + length]),
            }
            i += length;
            continue;
        }
        let c = source[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `escapeRegExpCharacters`.
pub fn escape_regexp(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if "-\\{}*+?|^$.,[]()#".contains(c) || c.is_whitespace() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Clone)]
pub struct RegExpSource {
    pub source: String,
    pub rule_id: i64,
    has_anchor: bool,
    pub has_back_references: bool,
    /// A0_G0, A0_G1, A1_G0, A1_G1.
    anchor_cache: Option<[String; 4]>,
}

impl RegExpSource {
    pub fn new(regex: &str, rule_id: i64) -> RegExpSource {
        let mut source = String::with_capacity(regex.len());
        let mut has_anchor = false;
        let mut chars = regex.chars();
        while let Some(c) = chars.next() {
            source.push(c);
            if c != '\\' {
                continue;
            }
            let Some(next) = chars.next() else { break };
            if next == 'z' {
                source.pop();
                source.push_str("$(?!\\n)(?<!\\n)");
                continue;
            }
            has_anchor |= next == 'A' || next == 'G';
            source.push(next);
        }
        let mut result = RegExpSource { has_back_references: has_back_references(&source), source, rule_id, has_anchor, anchor_cache: None };
        result.build_anchor_cache();
        result
    }

    pub fn set_source(&mut self, source: &str) {
        if self.source != source {
            self.source = source.to_string();
            self.build_anchor_cache();
        }
    }

    fn build_anchor_cache(&mut self) {
        if !self.has_anchor {
            return;
        }
        let mut variants: [String; 4] = Default::default();
        let mut chars = self.source.chars();
        while let Some(c) = chars.next() {
            variants.iter_mut().for_each(|v| v.push(c));
            if c != '\\' {
                continue;
            }
            let Some(next) = chars.next() else { break };
            let replaced = match next {
                'A' => ['\u{FFFF}', '\u{FFFF}', 'A', 'A'],
                'G' => ['\u{FFFF}', 'G', '\u{FFFF}', 'G'],
                _ => [next; 4],
            };
            for (variant, c) in variants.iter_mut().zip(replaced) {
                variant.push(c);
            }
        }
        self.anchor_cache = Some(variants);
    }

    fn resolve_anchors(&self, allow_a: bool, allow_g: bool) -> &str {
        match &self.anchor_cache {
            Some(variants) => &variants[(allow_a as usize) * 2 + allow_g as usize],
            None => &self.source,
        }
    }

    /// The `end` or `while` pattern with `\N` replaced by the escaped text of begin capture N.
    pub fn resolve_back_references(&self, line: &str, captures: &Captures) -> String {
        let mut out = String::new();
        let mut rest = self.source.as_str();
        while let Some(i) = rest.find('\\') {
            let digits = rest[i + 1..].bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                out.push_str(&rest[..i + 1]);
                rest = &rest[i + 1..];
                continue;
            }
            out.push_str(&rest[..i]);
            let index: usize = rest[i + 1..i + 1 + digits].parse().unwrap_or(usize::MAX);
            out.push_str(&escape_regexp(captures.get(index).map_or("", |&c| capture_text(line, c))));
            rest = &rest[i + 1 + digits..];
        }
        out.push_str(rest);
        out
    }
}

/// `/\\(\d+)/.test(source)`.
fn has_back_references(source: &str) -> bool {
    source.match_indices('\\').any(|(i, _)| source.as_bytes().get(i + 1).is_some_and(u8::is_ascii_digit))
}

#[derive(Default)]
pub struct RegExpSourceList {
    pub items: Vec<RegExpSource>,
    has_anchors: bool,
    /// Index 4 without anchors, else `A * 2 + G`.
    cached: [Option<Rc<Scanner>>; 5],
}

impl RegExpSourceList {
    pub fn push(&mut self, item: RegExpSource) {
        self.has_anchors |= item.has_anchor;
        self.items.push(item);
    }

    pub fn unshift(&mut self, item: RegExpSource) {
        self.has_anchors |= item.has_anchor;
        self.items.insert(0, item);
    }

    pub fn set_source(&mut self, index: usize, source: &str) {
        if self.items[index].source != source {
            self.cached = Default::default();
            self.items[index].set_source(source);
        }
    }

    pub fn compile_ag(&mut self, allow_a: bool, allow_g: bool) -> Result<Rc<Scanner>, onig::Error> {
        let slot = if self.has_anchors { (allow_a as usize) * 2 + allow_g as usize } else { 4 };
        if let Some(scanner) = &self.cached[slot] {
            return Ok(scanner.clone());
        }
        let sources = self.items.iter().map(|item| if self.has_anchors { item.resolve_anchors(allow_a, allow_g) } else { &item.source });
        let scanner = Rc::new(Scanner::new(sources, self.items.iter().map(|item| item.rule_id).collect())?);
        self.cached[slot] = Some(scanner.clone());
        Ok(scanner)
    }
}

/// vscode-oniguruma's `OnigScanner`: the leftmost match of all patterns, the first pattern on a tie.
pub struct Scanner {
    patterns: Vec<Pattern>,
    rule_ids: Vec<i64>,
}

struct Pattern {
    regex: Regex,
    /// Results depend on the search start only through `\G`; others reuse the last search on the same line.
    has_g_anchor: bool,
    last: RefCell<Option<CachedSearch>>,
}

struct CachedSearch {
    line_id: u64,
    from: usize,
    captures: Option<Captures>,
}

impl Scanner {
    fn new<'a>(sources: impl Iterator<Item = &'a str>, rule_ids: Vec<i64>) -> Result<Scanner, onig::Error> {
        let mut patterns = Vec::new();
        for source in sources {
            let regex = Regex::with_options(source, RegexOptions::REGEX_OPTION_CAPTURE_GROUP, Syntax::default())?;
            patterns.push(Pattern { regex, has_g_anchor: source.contains("\\G"), last: RefCell::new(None) });
        }
        Ok(Scanner { patterns, rule_ids })
    }

    /// `line_id` names the text: equal ids promise equal text, so cached searches stay valid.
    pub fn find_next_match(&self, line: &str, line_id: u64, from: usize) -> Option<(i64, Captures)> {
        let mut best: Option<(usize, Captures)> = None;
        for (index, pattern) in self.patterns.iter().enumerate() {
            let Some(captures) = pattern.search(line, line_id, from) else { continue };
            if best.as_ref().is_none_or(|(_, best)| captures[0].0 < best[0].0) {
                let at_start = captures[0].0 == from;
                best = Some((index, captures));
                if at_start {
                    break;
                }
            }
        }
        best.map(|(index, captures)| (self.rule_ids[index], captures))
    }
}

impl Pattern {
    fn search(&self, line: &str, line_id: u64, from: usize) -> Option<Captures> {
        if !self.has_g_anchor
            && let Some(last) = &*self.last.borrow()
            && last.line_id == line_id
            && last.from <= from
            && last.captures.as_ref().is_none_or(|captures| captures[0].0 >= from)
        {
            return last.captures.clone();
        }
        let mut region = Region::new();
        let found = self.regex.search_with_options(line, from, line.len(), SearchOptions::SEARCH_OPTION_NONE, Some(&mut region));
        let captures = found.map(|_| (0..region.len()).map(|i| region.pos(i).unwrap_or((usize::MAX, usize::MAX))).collect::<Captures>());
        *self.last.borrow_mut() = Some(CachedSearch { line_id, from, captures: captures.clone() });
        captures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_take_captures() {
        let line = "foo.bar baz";
        let captures = vec![(0, 11), (0, 7), (8, 11)];
        assert_eq!(replace_captures("entity.$1.${2:/upcase}.$9", line, &captures), "entity.foo.bar.BAZ.$9");
        assert!(has_captures("a.$1"));
        assert!(!has_captures("a.$b"));
    }

    #[test]
    fn anchors_and_back_references() {
        let source = RegExpSource::new("\\G(a)\\z\\1", 3);
        assert_eq!(source.source, "\\G(a)$(?!\\n)(?<!\\n)\\1");
        assert_eq!(source.resolve_anchors(false, false), "\\\u{FFFF}(a)$(?!\\n)(?<!\\n)\\1");
        assert_eq!(source.resolve_back_references("x.y", &vec![(0, 3), (0, 3)]), "\\G(a)$(?!\\n)(?<!\\n)x\\.y");
    }

    #[test]
    fn scanner_takes_the_leftmost_then_first_pattern() {
        let mut list = RegExpSourceList::default();
        list.push(RegExpSource::new("b+", 1));
        list.push(RegExpSource::new("a", 2));
        list.push(RegExpSource::new("ab", 3));
        let scanner = list.compile_ag(false, false).unwrap();
        assert_eq!(scanner.find_next_match("xxabbb", 1, 0), Some((2, vec![(2, 3)])));
        assert_eq!(scanner.find_next_match("xxabbb", 1, 3), Some((1, vec![(3, 6)])));
        assert_eq!(scanner.find_next_match("xxabbb", 1, 6), None);
    }
}
