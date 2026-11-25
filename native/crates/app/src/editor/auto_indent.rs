//! Auto-indentation from a language configuration: `onEnterRules` and brackets (onEnter.ts), `indentationRules`
//! (indentRules.ts) over lines with the brackets in strings, comments and regexes removed
//! (indentationLineProcessor.ts), electric brackets (electricCharacter.ts), and the indentation they give a line
//! (autoIndent.ts, enterAction.ts, contrib/indentation/common/indentation.ts).

use std::borrow::Cow;

use serde_json::Value;
use textmate::StandardTokenType;

use super::cursor::{self, Position};
use super::folding::marker_regex;
use super::edit::{ContentChanged, EditReason, end_of};
use super::{Indentation, OpenEditor, TextDocument, visible_column};
use crate::brackets::{bracket_pattern, character_pairs, escape_regex};
use crate::languages::{Languages, marker_pattern};
use crate::syntax::Tokenization;

/// `EditorAutoIndentStrategy`, ordered as upstream compares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strategy {
    None,
    Keep,
    Brackets,
    Advanced,
    Full,
}

impl Strategy {
    /// `editor.autoIndent`, "full" unless set to one of the other values.
    pub fn from_setting(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str) {
            Some("none") => Strategy::None,
            Some("keep") => Strategy::Keep,
            Some("brackets") => Strategy::Brackets,
            Some("advanced") => Strategy::Advanced,
            _ => Strategy::Full,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentAction {
    None,
    Indent,
    IndentOutdent,
    Outdent,
}

#[derive(Clone, Debug)]
struct EnterAction {
    indent: IndentAction,
    append_text: Option<String>,
    remove_text: usize,
}

/// `CompleteEnterAction`: the action with the text it appends and the indentation it keeps.
#[derive(Debug)]
pub struct CompleteEnterAction {
    pub indent: IndentAction,
    pub append_text: String,
    pub remove_text: usize,
    pub indentation: String,
}

struct OnEnterRule {
    before: onig::Regex,
    after: Option<onig::Regex>,
    previous: Option<onig::Regex>,
    action: EnterAction,
}

struct IndentRules {
    increase: onig::Regex,
    decrease: onig::Regex,
    indent_next: Option<onig::Regex>,
    unindented: Option<onig::Regex>,
}

/// `RichEditBrackets` as the electric character support reads it.
struct ElectricBrackets {
    chars: Vec<char>,
    regex: onig::Regex,
    opens: Vec<String>,
}

/// The onEnter, indentation and electric bracket rules of one language configuration.
pub struct LanguageRules {
    /// `OnEnterSupport`: the bracket pairs as (open, close) regexes and the `onEnterRules`; present when the
    /// configuration has brackets, indentation rules or onEnter rules.
    on_enter: Option<(Vec<(onig::Regex, onig::Regex)>, Vec<OnEnterRule>)>,
    indent: Option<IndentRules>,
    electric: Option<ElectricBrackets>,
    /// The last char of each auto-closing pair's open (`autoClosingPairsOpenByEnd`).
    auto_closing_open_ends: Vec<char>,
}

fn is_match(regex: &onig::Regex, text: &str) -> bool {
    regex.find(text).is_some()
}

/// `_parseRegex`, with the warning upstream logs for an invalid pattern.
fn parse_regex(language: &str, path: &str, value: &Value) -> Option<onig::Regex> {
    let Some((pattern, flags)) = marker_pattern(value) else {
        eprintln!("[{language}]: language configuration: expected `{path}` to be a string or an object.");
        return None;
    };
    marker_regex(pattern, flags).inspect_err(|error| eprintln!("[{language}]: Invalid regular expression in `{path}`: {error}")).ok()
}

impl LanguageRules {
    pub fn new(language: &str, config: &Value) -> Self {
        let brackets = character_pairs(&config["brackets"]);
        let indent = config["indentationRules"].is_object().then(|| indentation_rules(language, &config["indentationRules"])).flatten();
        let rules = on_enter_rules(language, &config["onEnterRules"]);
        let on_enter = (brackets.is_some() || config["indentationRules"].is_object() || rules.is_some()).then(|| {
            let default_brackets = || [("(", ")"), ("{", "}"), ("[", "]")].map(|(open, close)| (open.to_string(), close.to_string())).to_vec();
            let pairs = brackets.clone().unwrap_or_else(default_brackets).iter().map(|(open, close)| (open_bracket_regex(open), close_bracket_regex(close))).collect();
            (pairs, rules.unwrap_or_default())
        });
        let electric = brackets.as_ref().filter(|pairs| !pairs.is_empty()).map(|pairs| electric_brackets(pairs));
        let auto_closing = match config["autoClosingPairs"].as_array() {
            Some(pairs) => pairs.iter().filter_map(|pair| pair[0].as_str().or(pair["open"].as_str())).map(str::to_string).collect(),
            None => brackets.unwrap_or_default().into_iter().map(|(open, _)| open).collect::<Vec<_>>(),
        };
        let auto_closing_open_ends = auto_closing.iter().filter_map(|open| open.chars().last()).collect();
        LanguageRules { on_enter, indent, electric, auto_closing_open_ends }
    }

    /// `OnEnterSupport.onEnter`.
    fn on_enter(&self, strategy: Strategy, previous_line: &str, before: &str, after: &str) -> Option<EnterAction> {
        let (brackets, rules) = self.on_enter.as_ref()?;
        if strategy >= Strategy::Advanced {
            let matches = |regex: &Option<onig::Regex>, text| regex.as_ref().is_none_or(|regex| is_match(regex, text));
            let rule = rules.iter().find(|rule| is_match(&rule.before, before) && matches(&rule.after, after) && matches(&rule.previous, previous_line));
            if let Some(rule) = rule {
                return Some(rule.action.clone());
            }
        }
        if strategy >= Strategy::Brackets && !before.is_empty() {
            let action = |indent| Some(EnterAction { indent, append_text: None, remove_text: 0 });
            if !after.is_empty() && brackets.iter().any(|(open, close)| is_match(open, before) && is_match(close, after)) {
                return action(IndentAction::IndentOutdent);
            }
            if brackets.iter().any(|(open, _)| is_match(open, before)) {
                return action(IndentAction::Indent);
            }
        }
        None
    }

    /// `getElectricCharacters`.
    pub fn electric_chars(&self) -> &[char] {
        self.electric.as_ref().map_or(&[], |electric| &electric.chars)
    }
}

fn indentation_rules(language: &str, rules: &Value) -> Option<IndentRules> {
    let increase = parse_regex(language, "indentationRules.increaseIndentPattern", &rules["increaseIndentPattern"])?;
    let decrease = parse_regex(language, "indentationRules.decreaseIndentPattern", &rules["decreaseIndentPattern"])?;
    let optional = |key: &str| (!rules[key].is_null()).then(|| parse_regex(language, &format!("indentationRules.{key}"), &rules[key])).flatten();
    Some(IndentRules { increase, decrease, indent_next: optional("indentNextLinePattern"), unindented: optional("unIndentedLinePattern") })
}

/// `_extractValidOnEnterRules`: rules with an invalid action or `beforeText` are dropped.
fn on_enter_rules(language: &str, source: &Value) -> Option<Vec<OnEnterRule>> {
    if source.is_null() {
        return None;
    }
    let Some(source) = source.as_array() else {
        eprintln!("[{language}]: language configuration: expected `onEnterRules` to be an array.");
        return None;
    };
    let mut result = Vec::new();
    for (i, rule) in source.iter().enumerate() {
        let action = &rule["action"];
        let indent = match action["indent"].as_str() {
            Some("none") => IndentAction::None,
            Some("indent") => IndentAction::Indent,
            Some("indentOutdent") => IndentAction::IndentOutdent,
            Some("outdent") => IndentAction::Outdent,
            _ => {
                eprintln!("[{language}]: language configuration: expected `onEnterRules[{i}].action.indent` to be 'none', 'indent', 'indentOutdent' or 'outdent'.");
                continue;
            }
        };
        let append_text = action["appendText"].as_str().filter(|text| !text.is_empty()).map(str::to_string);
        let remove_text = action["removeText"].as_u64().unwrap_or(0) as usize;
        let Some(before) = parse_regex(language, &format!("onEnterRules[{i}].beforeText"), &rule["beforeText"]) else { continue };
        let optional = |key: &str| (!rule[key].is_null() && rule[key] != "").then(|| parse_regex(language, &format!("onEnterRules[{i}].{key}"), &rule[key])).flatten();
        let (after, previous) = (optional("afterText"), optional("previousLineText"));
        result.push(OnEnterRule { before, after, previous, action: EnterAction { indent, append_text, remove_text } });
    }
    (!result.is_empty()).then_some(result)
}

fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `_createOpenBracketRegExp`: the bracket at the end of the text before the cursor.
fn open_bracket_regex(bracket: &str) -> onig::Regex {
    let escaped = escape_regex(bracket);
    let boundary = if is_word_char(escaped.chars().next()) { "\\b" } else { "" };
    onig::Regex::new(&format!("{boundary}{escaped}\\s*$")).expect("escaped bracket compiles")
}

/// `_createCloseBracketRegExp`: the bracket at the start of the text after the cursor.
fn close_bracket_regex(bracket: &str) -> onig::Regex {
    let escaped = escape_regex(bracket);
    let boundary = if is_word_char(escaped.chars().last()) { "\\b" } else { "" };
    onig::Regex::new(&format!("^\\s*{escaped}{boundary}")).expect("escaped bracket compiles")
}

fn electric_brackets(pairs: &[(String, String)]) -> ElectricBrackets {
    let mut chars: Vec<char> = Vec::new();
    for (_, close) in pairs {
        let last = close.chars().last().expect("brackets are not empty");
        if !chars.contains(&last) {
            chars.push(last);
        }
    }
    let mut texts: Vec<&String> = pairs.iter().flat_map(|(open, close)| [open, close]).collect();
    texts.dedup();
    let pattern = texts.iter().map(|text| bracket_pattern(text)).collect::<Vec<_>>().join("|");
    let regex = onig::Regex::with_options(&pattern, onig::RegexOptions::REGEX_OPTION_IGNORECASE, onig::Syntax::default()).expect("escaped brackets compile");
    ElectricBrackets { chars, regex, opens: pairs.iter().map(|(open, _)| open.to_lowercase()).collect() }
}

/// `strings.getLeadingWhitespace`.
pub fn leading_whitespace(text: &str) -> &str {
    &text[..text.len() - text.trim_start_matches([' ', '\t']).len()]
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// `ShiftCommand.shiftIndent`/`unshiftIndent` of an indentation: one indent stop right or left.
pub fn shift_indent(indent: &str, indentation: Indentation, unshift: bool) -> String {
    cursor::shifted_indent(visible_column(indent, indent.chars().count(), indentation.tab_size), indentation, unshift)
}

fn byte_offset(line: &str, column: usize) -> usize {
    line.char_indices().nth(column).map_or(line.len(), |(offset, _)| offset)
}

/// A line's tokens as (start, end, standard type, language).
struct LineTokens(Vec<(usize, usize, StandardTokenType, String)>);

impl LineTokens {
    fn index_at(&self, offset: usize) -> usize {
        self.0.iter().position(|token| token.1 > offset).unwrap_or(self.0.len() - 1)
    }

    /// `createScopedLineTokens`: the bytes of the run of tokens in the language of the token at `offset`.
    fn scope(&self, offset: usize) -> (usize, usize, &str) {
        let index = self.index_at(offset);
        let language = &self.0[index].3;
        let first = self.0[..index].iter().rposition(|token| &token.3 != language).map_or(0, |before| before + 1);
        let last = self.0[index..].iter().position(|token| &token.3 != language).map_or(self.0.len(), |after| index + after) - 1;
        (self.0[first].0, self.0[last].1, language)
    }
}

/// The text model as the indentation rules read it: upstream's `IVirtualModel`, whose line `n` shows
/// document line `map(n)`, and whose `replaced` line has its own text and processed text.
pub struct Model<'a> {
    pub(super) lines: &'a [String],
    tokens: &'a Tokenization,
    language_id: &'a str,
    languages: &'a Languages,
    pub strategy: Strategy,
    pub indentation: Indentation,
    map: &'a dyn Fn(usize) -> usize,
    replaced: Option<(usize, String, String)>,
}

fn same_line(line: usize) -> usize {
    line
}

struct Inherit {
    indentation: String,
    action: Option<IndentAction>,
    line: Option<usize>,
}

impl<'a> Model<'a> {
    pub fn new(document: &'a TextDocument, languages: &'a Languages, strategy: Strategy) -> Self {
        Model { lines: &document.lines, tokens: &document.tokens, language_id: &document.language_id, languages, strategy, indentation: document.indentation, map: &same_line, replaced: None }
    }

    /// The same model with line `n` showing document line `map(n)`, and `replaced` = (line, text, processed
    /// text) over that.
    pub(super) fn virtual_model<'b>(&'b self, map: &'b dyn Fn(usize) -> usize, replaced: Option<(usize, String, String)>) -> Model<'b> {
        Model { lines: self.lines, tokens: self.tokens, language_id: self.language_id, languages: self.languages, strategy: self.strategy, indentation: self.indentation, map, replaced }
    }

    fn content(&self, line: usize) -> Cow<'_, str> {
        match &self.replaced {
            Some((replaced, text, _)) if *replaced == line => Cow::Borrowed(text),
            _ => Cow::Borrowed(&self.lines[(self.map)(line)]),
        }
    }

    fn rules(&self, language: &str) -> Option<&'a LanguageRules> {
        self.languages.rules(language)
    }

    fn indent_rules(&self, language: &str) -> Option<&'a IndentRules> {
        self.rules(language)?.indent.as_ref()
    }

    fn line_tokens(&self, line: usize) -> LineTokens {
        let mut start = 0;
        let tokens = self.tokens.line_tokens(self.lines, line).into_iter().map(|(end, token_type, language)| {
            let token = (start, end, token_type, language.map_or_else(|| self.language_id.to_string(), |language| language.to_string()));
            start = end;
            token
        });
        LineTokens(tokens.collect())
    }

    /// `getLanguageIdAtPosition` at char `column` of a document line, clamped to the document.
    pub(super) fn language_at(&self, line: usize, column: usize) -> String {
        let line = line.min(self.lines.len() - 1);
        let offset = byte_offset(&self.lines[line], column);
        self.tokens.language_at(self.lines, line, offset).map_or_else(|| self.language_id.to_string(), |language| language.to_string())
    }

    /// `IndentationLineProcessor.getProcessedTokens` over the bytes `from..to` of a document line: the
    /// brackets in its string, regex and comment tokens removed.
    fn processed_slice(&self, line: usize, from: usize, to: usize, tokens: &LineTokens) -> String {
        let text = &self.lines[line];
        let language = &tokens.0[tokens.index_at(from)].3;
        let brackets = self.languages.brackets.get(language.as_str());
        let mut result = String::new();
        for &(start, end, token_type, _) in &tokens.0 {
            let (start, end) = (start.max(from), end.min(to));
            if start >= end {
                continue;
            }
            let piece = &text[start..end];
            match (token_type, brackets) {
                (StandardTokenType::String | StandardTokenType::RegEx | StandardTokenType::Comment, Some(brackets)) => result.push_str(&brackets.strip(piece)),
                _ => result.push_str(piece),
            }
        }
        result
    }

    pub(super) fn processed_line(&self, line: usize) -> String {
        self.processed_slice(line, 0, self.lines[line].len(), &self.line_tokens(line))
    }

    /// `IndentationLineProcessor.getProcessedLine`, its indentation replaced by `indentation` when given.
    fn processed(&self, line: usize, indentation: Option<&str>) -> String {
        let processed = match &self.replaced {
            Some((replaced, _, processed)) if *replaced == line => processed.clone(),
            _ => self.processed_line((self.map)(line)),
        };
        match indentation {
            Some(indentation) => format!("{indentation}{}", &processed[leading_whitespace(&processed).len()..]),
            None => processed,
        }
    }

    /// `isLanguageDifferentFromLineStart`.
    fn language_differs_from_line_start(&self, position: Position) -> bool {
        let tokens = self.line_tokens(position.line);
        let (first, _, language) = tokens.scope(byte_offset(&self.lines[position.line], position.column));
        first != 0 && tokens.0[0].3 != language
    }

    /// `IndentationContextProcessor.getProcessedTokenContextAroundRange`: the processed text before the
    /// range, after it, and of the previous line, each within the language at the range start.
    fn context_around(&self, start: Position, end: Position) -> (String, String, String) {
        let tokens = self.line_tokens(start.line);
        let offset = byte_offset(&self.lines[start.line], start.column);
        let (scope_start, _, language) = tokens.scope(offset);
        let from = if self.language_differs_from_line_start(start) { scope_start } else { 0 };
        let before = self.processed_slice(start.line, from, offset, &tokens);
        let end_tokens = self.line_tokens(end.line);
        let end_offset = byte_offset(&self.lines[end.line], end.column);
        let (_, end_scope_end, _) = end_tokens.scope(end_offset);
        let after = self.processed_slice(end.line, end_offset, end_scope_end, &end_tokens);
        let mut previous = String::new();
        if start.line > 0 && scope_start == 0 {
            let previous_tokens = self.line_tokens(start.line - 1);
            let (previous_start, previous_end, previous_language) = previous_tokens.scope(self.lines[start.line - 1].len());
            if previous_language == language {
                previous = self.processed_slice(start.line - 1, previous_start, previous_end, &previous_tokens);
            }
        }
        (before, after, previous)
    }

    /// `getPrecedingValidLine`: the nearest line above that the rules do not ignore and that is not blank.
    fn preceding_valid_line(&self, line: usize, rules: &IndentRules) -> Option<usize> {
        let language = self.language_at(line, 0);
        let mut result = None;
        for last in (0..line).rev() {
            if self.language_at(last, 0) != language {
                return result;
            }
            if should(&rules.unindented, &self.processed(last, None)) || is_blank(&self.content(last)) {
                result = Some(last);
                continue;
            }
            return Some(last);
        }
        None
    }

    /// `getInheritIndentForLine`: the indentation line `line` inherits from the lines above, and whether it indents.
    fn inherit_indent(&self, line: usize, honor_intentional_indent: bool) -> Option<Inherit> {
        if self.strategy < Strategy::Full {
            return None;
        }
        let rules = self.indent_rules(self.language_id)?;
        let at = |line: usize, action| Some(Inherit { indentation: leading_whitespace(&self.content(line)).to_string(), action, line: Some(line) });
        let empty = || Some(Inherit { indentation: String::new(), action: None, line: None });
        if line == 0 {
            return empty();
        }
        for prior in (0..line).rev() {
            if !self.content(prior).is_empty() {
                break;
            }
            if prior == 0 {
                return empty();
            }
        }
        let preceding = self.preceding_valid_line(line, rules)?;
        let increases = |line| is_match(&rules.increase, &self.processed(line, None));
        let indents_next = |line| should(&rules.indent_next, &self.processed(line, None));
        let decreases = |line| is_match(&rules.decrease, &self.processed(line, None));
        if increases(preceding) || indents_next(preceding) {
            return at(preceding, Some(IndentAction::Indent));
        }
        if decreases(preceding) {
            return at(preceding, None);
        }
        if preceding == 0 {
            return at(0, None);
        }
        let previous = self.content(preceding - 1);
        let previous_increases_or_decreases = is_match(&rules.increase, &previous) || is_match(&rules.decrease, &previous);
        if !previous_increases_or_decreases && should(&rules.indent_next, &previous) {
            let stop = (0..preceding - 1).rev().find(|&line| !indents_next(line));
            return at(stop.map_or(0, |stop| stop + 1), None);
        }
        if honor_intentional_indent {
            return at(preceding, None);
        }
        for line in (0..=preceding).rev() {
            if increases(line) {
                return at(line, Some(IndentAction::Indent));
            }
            // Upstream's inner loop tests line `i` instead of `j`, so it always ends on the first line.
            if indents_next(line) {
                return at(0, None);
            }
            if decreases(line) {
                return at(line, None);
            }
        }
        at(0, None)
    }

    /// `getGoodIndentForLine`.
    pub fn good_indent(&self, language: &str, line: usize) -> Option<String> {
        if self.strategy < Strategy::Full {
            return None;
        }
        let rules = self.rules(language)?;
        let indent_rules = rules.indent.as_ref()?;
        let indent = self.inherit_indent(line, true)?;
        let shift = |indent: &str, unshift| shift_indent(indent, self.indentation, unshift);
        let decreases = is_match(&indent_rules.decrease, &self.processed(line, None));
        if let Some(inherit_line) = indent.line
            && (inherit_line..line.saturating_sub(1)).all(|between| is_blank(&self.content(between)))
            && let Some(enter) = rules.on_enter(self.strategy, "", &self.content(inherit_line), "")
        {
            let mut indentation = leading_whitespace(&self.content(inherit_line)).to_string();
            indentation.truncate(indentation.len().saturating_sub(enter.remove_text));
            match enter.indent {
                IndentAction::Indent | IndentAction::IndentOutdent => indentation = shift(&indentation, false),
                IndentAction::Outdent => indentation = shift(&indentation, true),
                IndentAction::None => {}
            }
            if decreases {
                indentation = shift(&indentation, true);
            }
            indentation += enter.append_text.as_deref().unwrap_or("");
            return Some(leading_whitespace(&indentation).to_string());
        }
        let indents = indent.action == Some(IndentAction::Indent);
        Some(match (decreases, indents) {
            (true, true) | (false, false) => indent.indentation,
            (true, false) => shift(&indent.indentation, true),
            (false, true) => shift(&indent.indentation, false),
        })
    }

    /// `getEnterAction` for a line break replacing `start..end`.
    pub fn enter_action(&self, start: Position, end: Position) -> Option<CompleteEnterAction> {
        let rules = self.rules(&self.language_at(start.line, start.column))?;
        let (before, after, previous) = self.context_around(start, end);
        let enter = rules.on_enter(self.strategy, &previous, &before, &after)?;
        let append_text = match (enter.append_text, enter.indent) {
            (None, IndentAction::Indent | IndentAction::IndentOutdent) => "\t".to_string(),
            (None, _) => String::new(),
            (Some(text), IndentAction::Indent) => format!("\t{text}"),
            (Some(text), _) => text,
        };
        let mut indentation = self.indentation_at(start);
        indentation.truncate(indentation.len().saturating_sub(enter.remove_text));
        Some(CompleteEnterAction { indent: enter.indent, append_text, remove_text: enter.remove_text, indentation })
    }

    /// `getIndentationAtPosition`: the line's indentation, cut at the position.
    fn indentation_at(&self, position: Position) -> String {
        leading_whitespace(&self.lines[position.line]).chars().take(position.column).collect()
    }

    /// `getIndentForEnter`: the indentation of the new line after a line break replacing `start..end`.
    pub fn indent_for_enter(&self, start: Position, end: Position) -> Option<String> {
        if self.strategy < Strategy::Full {
            return None;
        }
        let rules = self.indent_rules(&self.language_at(start.line, start.column))?;
        let (before, after, _) = self.context_around(start, end);
        let before_indent = leading_whitespace(&before).to_string();
        // `createVirtualModelWithModifiedTokensAtLine`: the line reads as its processed text before the range.
        let virtual_model = self.virtual_model(&same_line, Some((start.line, before.clone(), before.clone())));
        let differs = self.language_differs_from_line_start(start);
        let current_indent = leading_whitespace(&self.lines[start.line]).to_string();
        let Some(inherit) = virtual_model.inherit_indent(start.line + 1, true) else {
            return Some(if differs { current_indent } else { before_indent });
        };
        let mut indent = if differs { current_indent } else { inherit.indentation };
        if inherit.action == Some(IndentAction::Indent) {
            indent = shift_indent(&indent, self.indentation, false);
        }
        if is_match(&rules.decrease, &after) {
            indent = shift_indent(&indent, self.indentation, true);
        }
        Some(indent)
    }

    /// `getIndentActionForType`: the indentation typing `ch` over `start..end` gives its line, if it changes.
    pub fn indent_for_type(&self, start: Position, end: Position, ch: &str) -> Option<String> {
        if self.strategy < Strategy::Full || self.language_differs_from_line_start(start) {
            return None;
        }
        let language = self.language_at(start.line, start.column);
        let rules = self.rules(&language)?;
        let indent_rules = rules.indent.as_ref()?;
        let (before, after, _) = self.context_around(start, end);
        let around = format!("{before}{after}");
        let with_char = format!("{before}{ch}{after}");
        if !is_match(&indent_rules.decrease, &around) && is_match(&indent_rules.decrease, &with_char) {
            let inherit = self.inherit_indent(start.line, false)?;
            return Some(match inherit.action {
                Some(IndentAction::Indent) => inherit.indentation,
                _ => shift_indent(&inherit.indentation, self.indentation, true),
            });
        }
        if start == end && start.line > 0 && should(&indent_rules.indent_next, &self.lines[start.line - 1]) && is_match(&indent_rules.increase, &with_char) {
            let inherited = self.inherit_indent(start.line, false)?.indentation;
            let inferred = shift_indent(&inherited, self.indentation, false);
            let auto_closes = ch.chars().last().is_some_and(|last| rules.auto_closing_open_ends.contains(&last));
            if inferred == leading_whitespace(&self.lines[start.line]) && auto_closes && is_blank(&around) {
                return Some(inherited);
            }
        }
        None
    }

    /// `BracketElectricCharacterSupport.onElectricCharacter`: the closing bracket that typing `ch` at
    /// `position` completes as the first text of its line.
    pub fn electric_bracket(&self, position: Position, ch: &str) -> Option<String> {
        let line = &self.lines[position.line];
        let offset = byte_offset(line, position.column);
        let tokens = self.line_tokens(position.line);
        let (scope_start, _, language) = tokens.scope(offset);
        let electric = self.rules(language)?.electric.as_ref()?;
        if matches!(tokens.0[tokens.index_at(offset)].2, StandardTokenType::String | StandardTokenType::RegEx | StandardTokenType::Comment) {
            return None;
        }
        let text = format!("{}{ch}", &line[scope_start..offset]);
        let (start, end) = electric.regex.find_iter(&text).last()?;
        let bracket = text[start..end].to_lowercase();
        if electric.opens.contains(&bracket) || !is_blank(&line[..scope_start + start]) {
            return None;
        }
        Some(bracket)
    }

    /// `getReindentEditOperations`: (line, indentation length in chars, new indentation) for the lines
    /// `start..=end` reindented by the rules, from the first line the rules do not ignore.
    pub fn reindent_edits(&self, mut start: usize, end: usize) -> Vec<(usize, usize, String)> {
        if self.lines.len() == 1 && self.lines[0].is_empty() {
            return Vec::new();
        }
        let Some(rules) = self.indent_rules(self.language_id) else { return Vec::new() };
        let end = end.min(self.lines.len() - 1);
        while start <= end && should(&rules.unindented, &self.processed(start, None)) {
            start += 1;
        }
        if start + 1 > end {
            return Vec::new();
        }
        let shift = |indent: &str, unshift| shift_indent(indent, self.indentation, unshift);
        let mut global_indent = leading_whitespace(&self.lines[start]).to_string();
        let mut ideal_indent = global_indent.clone();
        let first = self.processed(start, None);
        if is_match(&rules.increase, &first) {
            ideal_indent = shift(&ideal_indent, false);
            global_indent = shift(&global_indent, false);
        } else if should(&rules.indent_next, &first) {
            ideal_indent = shift(&ideal_indent, false);
        }
        let mut edits = Vec::new();
        for line in start + 1..=end {
            if self.line_tokens(line).0[0].2 == StandardTokenType::String {
                continue;
            }
            let old_indent = leading_whitespace(&self.lines[line]);
            let current_ideal = ideal_indent.clone();
            let processed = self.processed(line, Some(&current_ideal));
            if is_match(&rules.decrease, &processed) {
                ideal_indent = shift(&ideal_indent, true);
                global_indent = shift(&global_indent, true);
            }
            if old_indent != ideal_indent {
                edits.push((line, old_indent.len(), cursor::normalize_indentation(&ideal_indent, self.indentation)));
            }
            if should(&rules.unindented, &self.processed(line, None)) {
                continue;
            }
            if is_match(&rules.increase, &processed) {
                global_indent = shift(&global_indent, false);
                ideal_indent = global_indent.clone();
            } else if should(&rules.indent_next, &processed) {
                ideal_indent = shift(&ideal_indent, false);
            } else {
                ideal_indent = global_indent.clone();
            }
        }
        edits
    }
}

fn should(regex: &Option<onig::Regex>, text: &str) -> bool {
    regex.as_ref().is_some_and(|regex| is_match(regex, text))
}

/// What the cursor's type operations read from `CursorConfiguration`: the language configurations and `editor.autoIndent`.
#[derive(Clone, Copy)]
pub struct AutoIndent<'a> {
    pub languages: &'a Languages,
    pub strategy: Strategy,
}

impl<'a> AutoIndent<'a> {
    pub fn model(self, document: &'a TextDocument) -> Model<'a> {
        Model::new(document, self.languages, self.strategy)
    }
}

impl OpenEditor {
    /// `ReindentLinesAction.run`: every line reindented by the language's indentation rules, as one undo step.
    pub fn reindent_lines(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        let edits = auto.model(&self.document).reindent_edits(0, self.document.lines.len() - 1);
        self.reindent(edits, "editor.action.reindentlines")
    }

    /// `ReindentSelectedLinesAction.run`: each selection's lines reindented, from the line above it.
    pub fn reindent_selected_lines(&mut self, auto: AutoIndent) -> Option<ContentChanged> {
        let model = auto.model(&self.document);
        let mut edits = Vec::new();
        for selection in &self.selections {
            let (mut start, mut end) = (selection.start().line, selection.end().line);
            if start != end && selection.end().column == 0 {
                end -= 1;
            }
            if start == 0 && start == end {
                continue;
            }
            start = start.saturating_sub(1);
            edits.extend(model.reindent_edits(start, end));
        }
        self.reindent(edits, "editor.action.reindentselectedlines")
    }

    fn reindent(&mut self, edits: Vec<(usize, usize, String)>, id: &'static str) -> Option<ContentChanged> {
        if edits.is_empty() {
            return None;
        }
        let edits = edits.into_iter().map(|(line, indent_len, indent)| (Position::new(line, 0), Position::new(line, indent_len), indent, false)).collect();
        self.push_undo_stop();
        let change = self.execute_edits(edits, None, EditReason::Unknown(id));
        self.push_undo_stop();
        change
    }
}

/// `indentUtils.getSpaceCnt`.
pub(super) fn space_count(indent: &str, tab_size: usize) -> usize {
    indent.chars().map(|c| if c == '\t' { tab_size } else { 1 }).sum()
}

/// `indentUtils.generateIndent`.
pub(super) fn generate_indent(count: isize, indentation: Indentation) -> String {
    let mut count = count.max(0) as usize;
    let mut result = String::new();
    if !indentation.insert_spaces {
        result = "\t".repeat(count / indentation.tab_size);
        count %= indentation.tab_size;
    }
    result + &" ".repeat(count)
}

/// The moveLinesCommand.ts helpers that ask the onEnter and indentation rules.
impl Model<'_> {
    /// `shouldAutoIndent`: the moved lines are in one language that has indentation rules.
    pub(super) fn indents_moved_lines(&self, first: usize, last: usize) -> bool {
        let language = self.language_at(first, 0);
        self.strategy >= Strategy::Full && language == self.language_at(last, 0) && self.indent_rules(&language).is_some()
    }

    /// `matchEnterRule`: the indent offset Enter at the end of the nearest non-blank line from
    /// `one_line_above` up gives `line`.
    pub(super) fn enter_rule_offset(&self, line: usize, one_line_above: Option<usize>) -> Option<isize> {
        let above = (0..=one_line_above?).rev().find(|&above| !is_blank(&self.lines[above]))?;
        self.enter_offset(line, self.enter_at_end(above))
    }

    /// `matchEnterRuleMovingDown`: as `enter_rule_offset`, from the line that will be above `line`.
    pub(super) fn enter_rule_offset_moving_down(&self, line: usize, future_above: usize, future_above_text: &str) -> Option<isize> {
        if !is_blank(future_above_text) {
            return self.enter_offset(line, self.enter_at_end(future_above));
        }
        let above = (0..line).rev().find(|&above| !is_blank(&self.lines[above]))?;
        self.enter_offset(line, self.enter_at_end(above))
    }

    /// `ShiftCommand`'s miss-aligned line: when `line`'s content is off the indent stops, the spaces the
    /// onEnter rule of the line above appended (a JSDoc ` * `) do not count as its indentation.
    pub(super) fn shift_extra_spaces(&self, line: usize, previous_extra: usize) -> usize {
        let text = &self.lines[line];
        let indent_end = first_non_whitespace(text).unwrap_or(text.chars().count());
        let size = self.indentation.indent_size();
        if line == 0 || visible_column(text, indent_end, self.indentation.tab_size) % size == 0 {
            return 0;
        }
        let Some(enter) = self.enter_at_end(line - 1) else { return 0 };
        let mut extra = previous_extra;
        for c in enter.append_text.chars() {
            if c != ' ' || extra >= size {
                break;
            }
            extra += 1;
        }
        extra.saturating_sub(enter.remove_text)
    }

    fn enter_at_end(&self, line: usize) -> Option<CompleteEnterAction> {
        let end = Position::new(line, self.lines[line].chars().count());
        self.enter_action(end, end)
    }

    /// `parseEnterResult`: how many columns the enter action moves `line`'s indentation, if `line` holds its prefix.
    fn enter_offset(&self, line: usize, enter: Option<CompleteEnterAction>) -> Option<isize> {
        let enter = enter?;
        let prefix = match enter.indent {
            IndentAction::None | IndentAction::Indent => enter.indentation + &enter.append_text,
            IndentAction::IndentOutdent => enter.indentation,
            IndentAction::Outdent => shift_indent(&enter.indentation, self.indentation, true) + &enter.append_text,
        };
        let text = &self.lines[line];
        if !text.trim_start().contains(prefix.trim_start()) {
            return None;
        }
        let mut indent = leading_whitespace(&prefix).to_string();
        // `getIndentMetadata` of the model's language.
        if self.indent_rules(self.language_id).is_some_and(|rules| is_match(&rules.decrease, text)) {
            indent = shift_indent(&indent, self.indentation, true);
        }
        let tab_size = self.indentation.tab_size;
        Some(space_count(&indent, tab_size) as isize - space_count(leading_whitespace(text), tab_size) as isize)
    }
}

/// A replace of `start..end` by `text` that leaves the caret at `caret`.
pub type Edit = (Position, Position, String, Position);

/// `strings.firstNonWhitespaceIndex` in chars.
fn first_non_whitespace(line: &str) -> Option<usize> {
    line.chars().position(|c| c != ' ' && c != '\t')
}

/// `line.substring(a, b)` in chars, which swaps its bounds when `a > b`.
fn substring(line: &str, a: usize, b: usize) -> String {
    line.chars().skip(a.min(b)).take(a.abs_diff(b)).collect()
}

impl Model<'_> {
    fn normalize(&self, indent: &str) -> String {
        cursor::normalize_indentation(indent, self.indentation)
    }

    /// `EnterOperation._enter` without `keepPosition`: a line break replacing `start..end`, indented by
    /// the onEnter rules, else by the indentation rules, else as the line.
    pub fn enter_edit(&self, start: Position, end: Position) -> Edit {
        let typed = |text: String| {
            let caret = end_of(start, &text);
            (start, end, text, caret)
        };
        if self.strategy == Strategy::None {
            return typed("\n".into());
        }
        let indentation = self.indentation_at(start);
        if self.strategy == Strategy::Keep {
            return typed(format!("\n{}", self.normalize(&indentation)));
        }
        if let Some(action) = self.enter_action(start, end) {
            return match action.indent {
                IndentAction::None | IndentAction::Indent => typed(format!("\n{}", self.normalize(&(action.indentation + &action.append_text)))),
                IndentAction::IndentOutdent => {
                    let normal = self.normalize(&action.indentation);
                    let increased = self.normalize(&(action.indentation + &action.append_text));
                    let caret = Position::new(start.line + 1, increased.chars().count());
                    (start, end, format!("\n{increased}\n{normal}"), caret)
                }
                IndentAction::Outdent => typed(format!("\n{}", self.normalize(&(shift_indent(&action.indentation, self.indentation, true) + &action.append_text)))),
            };
        }
        let Some(after) = self.indent_for_enter(start, end) else { return typed(format!("\n{}", self.normalize(&indentation))) };
        // The whitespace after the cursor goes, and a cursor inside the indentation keeps its visible column.
        let line = &self.lines[end.line];
        let first = first_non_whitespace(line);
        let end_after = Position::new(end.line, first.map_or(line.chars().count(), |first| end.column.max(first)));
        let after = self.normalize(&after);
        let mut offset = 0;
        if first.is_some_and(|first| end.column <= first) {
            let mut visible = visible_column(line, end.column, self.indentation.tab_size);
            if !self.indentation.insert_spaces {
                visible = visible.div_ceil(self.indentation.indent_size());
            }
            offset = visible.min(after.len()) as isize - after.len() as isize;
        }
        let text = format!("\n{after}");
        let caret = end_of(start, &text);
        (start, end_after, text, Position::new(caret.line, caret.column.saturating_add_signed(offset)))
    }

    /// `AutoIndentOperation` for one cursor: typing `ch` over `start..end` with the line reindented, or
    /// `None` when the indentation stays.
    pub fn type_edit(&self, start: Position, end: Position, ch: &str) -> Option<Edit> {
        let indentation = self.indent_for_type(start, end, ch)?;
        if indentation == self.normalize(&self.indentation_at(start)) {
            return None;
        }
        let line = &self.lines[start.line];
        let mut text = self.normalize(&indentation);
        if let Some(first) = first_non_whitespace(line) {
            text += &substring(line, first, start.column);
        }
        text += ch;
        let caret = end_of(Position::new(start.line, 0), &text);
        Some((Position::new(start.line, 0), end, text, caret))
    }

    /// `InterceptorElectricCharOperation`: a closing bracket typed as the first text of its line takes
    /// the indentation of the line of its opening bracket.
    pub fn electric_edit(&self, position: Position, ch: &str) -> Option<Edit> {
        let typed = ch.chars().next()?;
        if !self.rules(self.language_id).is_some_and(|rules| rules.electric_chars().contains(&typed)) {
            return None;
        }
        let bracket = self.electric_bracket(position, ch)?;
        let line = &self.lines[position.line];
        let bracket_start = format!("{line}{ch}").rfind(&bracket)?;
        let language = self.language_at(position.line, position.column);
        let open_line = self.tokens.matching_bracket_up(self.languages, self.lines, &language, &bracket, position.line, bracket_start)?;
        if open_line == position.line {
            return None;
        }
        let mut text = self.normalize(leading_whitespace(&self.lines[open_line]));
        text += &substring(line, first_non_whitespace(line).unwrap_or(position.column), position.column);
        text += ch;
        let start = Position::new(position.line, 0);
        Some((start, position, text.clone(), end_of(start, &text)))
    }

    /// `TabOperation._goodIndentForLine`: the indentation Tab gives a blank line.
    pub fn tab_indent(&self, line: usize) -> Option<String> {
        let (action, mut indentation) = match self.inherit_indent(line, false) {
            Some(inherit) => (inherit.action, inherit.indentation),
            None => {
                let above = (0..line).rev().find(|&above| !is_blank(&self.lines[above]))?;
                let end = Position::new(above, self.lines[above].chars().count());
                let indentation = self.enter_action(end, end).map(|action| action.indentation + &action.append_text).unwrap_or_default();
                (None, indentation)
            }
        };
        if let Some(action) = action {
            match action {
                IndentAction::Indent => indentation = shift_indent(&indentation, self.indentation, false),
                IndentAction::Outdent => indentation = shift_indent(&indentation, self.indentation, true),
                _ => {}
            }
            indentation = self.normalize(&indentation);
        }
        (!indentation.is_empty()).then_some(indentation)
    }
}

#[cfg(test)]
mod tests;
