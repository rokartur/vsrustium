//! A compiled grammar and line tokenization, ported from vscode-textmate 9.3.2 (grammar.ts,
//! tokenizeString.ts, basicScopesAttributeProvider.ts). Offsets are UTF-8 byte offsets in the line.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::matcher::{Matcher, create_matchers};
use crate::raw::{RawCaptures, RawGrammar, RawRule, Repository};
use crate::rule::{Captures, END_RULE_ID, RegExpSource, RegExpSourceList, Rule, RuleKind, Scanner, WHILE_RULE_ID, escape_regexp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardTokenType {
    Other,
    Comment,
    String,
    RegEx,
}

/// A scope path, innermost last, with the attributes vscode-textmate encodes next to it: the
/// embedded language and standard token type of the innermost scope that sets them.
pub struct ScopeStack {
    pub parent: Option<Rc<ScopeStack>>,
    pub scope_name: String,
    pub language: Option<Rc<str>>,
    pub token_type: Option<StandardTokenType>,
}

impl ScopeStack {
    pub fn from_names(names: &[&str]) -> Rc<ScopeStack> {
        let mut stack: Option<Rc<ScopeStack>> = None;
        for name in names {
            stack = Some(Rc::new(ScopeStack { parent: stack, scope_name: name.to_string(), language: None, token_type: None }));
        }
        stack.expect("at least one scope")
    }

    /// Root first.
    pub fn names(&self) -> Vec<&str> {
        let mut names = vec![self.scope_name.as_str()];
        let mut current = self.parent.as_deref();
        while let Some(stack) = current {
            names.push(&stack.scope_name);
            current = stack.parent.as_deref();
        }
        names.reverse();
        names
    }
}

pub struct Token {
    pub start: usize,
    pub end: usize,
    pub scopes: Rc<ScopeStack>,
    /// After the grammar's `tokenTypes` overrides.
    pub token_type: StandardTokenType,
    /// The token may hold brackets that pair up, per `balancedBracketScopes` and `unbalancedBracketScopes`.
    pub balanced_brackets: bool,
}

/// The grammar contribution settings `TMGrammarFactory.createGrammar` passes on.
#[derive(Clone, Default)]
pub struct GrammarConfiguration {
    pub language: Option<String>,
    pub embedded_languages: Vec<(String, String)>,
    pub token_types: Vec<(String, StandardTokenType)>,
    pub balanced_bracket_selectors: Vec<String>,
    pub unbalanced_bracket_selectors: Vec<String>,
}

/// Where a grammar finds the grammars it includes and the grammars injected into a scope.
pub trait GrammarLookup {
    fn raw_grammar(&self, scope_name: &str, base: Option<Rc<RawRule>>) -> Option<RawGrammar>;
    fn injections(&self, scope_name: &str) -> Vec<String>;
}

struct Injection {
    matcher: Matcher,
    priority: i32,
    rule_id: usize,
}

pub struct Grammar {
    rules: Vec<Option<Rule>>,
    root_id: usize,
    injections: Vec<Injection>,
    language: Option<Rc<str>>,
    /// Longest scope first, as `ScopeMatcher` sorts its regex alternatives.
    embedded_languages: Vec<(String, Rc<str>)>,
    token_types: Vec<(Matcher, StandardTokenType)>,
    balanced_brackets: Vec<Matcher>,
    unbalanced_brackets: Vec<Matcher>,
    balanced_allow_any: bool,
    next_line_id: Cell<u64>,
}

/// One element of the rule stack; the state between lines is the innermost one.
pub struct StateStack {
    parent: Option<Rc<StateStack>>,
    rule_id: usize,
    enter_pos: Cell<isize>,
    anchor_pos: Cell<isize>,
    begin_rule_captured_eol: bool,
    end_rule: Option<String>,
    name_scopes: Rc<ScopeStack>,
    content_name_scopes: Rc<ScopeStack>,
}

pub type State = Rc<StateStack>;

impl StateStack {
    fn push(self: &Rc<Self>, rule_id: usize, enter_pos: isize, anchor_pos: isize, begin_rule_captured_eol: bool, end_rule: Option<String>, name_scopes: Rc<ScopeStack>, content_name_scopes: Rc<ScopeStack>) -> State {
        Rc::new(StateStack { parent: Some(self.clone()), rule_id, enter_pos: Cell::new(enter_pos), anchor_pos: Cell::new(anchor_pos), begin_rule_captured_eol, end_rule, name_scopes, content_name_scopes })
    }

    fn with_content_name_scopes(self: &Rc<Self>, content_name_scopes: Rc<ScopeStack>) -> State {
        if Rc::ptr_eq(&self.content_name_scopes, &content_name_scopes) {
            return self.clone();
        }
        self.parent.as_ref().unwrap().push(self.rule_id, self.enter_pos.get(), self.anchor_pos.get(), self.begin_rule_captured_eol, self.end_rule.clone(), self.name_scopes.clone(), content_name_scopes)
    }

    fn with_end_rule(self: &Rc<Self>, end_rule: String) -> State {
        if self.end_rule.as_deref() == Some(end_rule.as_str()) {
            return self.clone();
        }
        Rc::new(StateStack {
            parent: self.parent.clone(),
            rule_id: self.rule_id,
            enter_pos: Cell::new(self.enter_pos.get()),
            anchor_pos: Cell::new(self.anchor_pos.get()),
            begin_rule_captured_eol: self.begin_rule_captured_eol,
            end_rule: Some(end_rule),
            name_scopes: self.name_scopes.clone(),
            content_name_scopes: self.content_name_scopes.clone(),
        })
    }

    fn safe_pop(self: &Rc<Self>) -> State {
        self.parent.clone().unwrap_or_else(|| self.clone())
    }

    fn has_same_rule_as(&self, other: &StateStack) -> bool {
        let mut element = Some(self);
        while let Some(current) = element.filter(|e| e.enter_pos.get() == other.enter_pos.get()) {
            if current.rule_id == other.rule_id {
                return true;
            }
            element = current.parent.as_deref();
        }
        false
    }

    fn reset(&self) {
        let mut element = Some(self);
        while let Some(current) = element {
            current.enter_pos.set(-1);
            current.anchor_pos.set(-1);
            element = current.parent.as_deref();
        }
    }

    /// `StateStackImpl.equals`: same rules and end rules all the way down and the same scopes on top,
    /// so tokenizing the next line gives the same result.
    pub fn same_as(&self, other: &StateStack) -> bool {
        let (mut a, mut b) = (Some(self), Some(other));
        while let (Some(x), Some(y)) = (a, b) {
            if std::ptr::eq(x, y) {
                break;
            }
            if x.rule_id != y.rule_id || x.end_rule != y.end_rule {
                return false;
            }
            a = x.parent.as_deref();
            b = y.parent.as_deref();
        }
        if a.is_some() != b.is_some() {
            return false;
        }
        let (mut a, mut b) = (Some(&*self.content_name_scopes), Some(&*other.content_name_scopes));
        while let (Some(x), Some(y)) = (a, b) {
            if std::ptr::eq(x, y) {
                return true;
            }
            if x.scope_name != y.scope_name || x.language != y.language || x.token_type != y.token_type {
                return false;
            }
            a = x.parent.as_deref();
            b = y.parent.as_deref();
        }
        a.is_none() && b.is_none()
    }
}

impl Grammar {
    /// `Registry.loadGrammarWithConfiguration`: compiles every rule reachable from the grammar and its injections.
    pub fn new(raw: RawGrammar, configuration: &GrammarConfiguration, lookup: &dyn GrammarLookup) -> Grammar {
        let mut embedded_languages: Vec<(String, Rc<str>)> = configuration.embedded_languages.iter().map(|(scope, language)| (scope.clone(), Rc::from(language.as_str()))).collect();
        embedded_languages.sort_by(|a, b| escape_regexp(&b.0).cmp(&escape_regexp(&a.0)));
        let mut token_types = Vec::new();
        for (selector, token_type) in &configuration.token_types {
            for matcher in create_matchers(selector) {
                token_types.push((matcher.matcher, *token_type));
            }
        }
        let balanced_allow_any = configuration.balanced_bracket_selectors.iter().any(|s| s == "*");
        let matchers = |selectors: &[String]| selectors.iter().filter(|s| *s != "*").flat_map(|s| create_matchers(s)).map(|m| m.matcher).collect::<Vec<_>>();
        let mut builder = Builder { rules: vec![None], included: HashMap::new(), lookup };
        let root_id = builder.compiled_rule_id(&raw.repository["$self"], &raw.repository);
        let mut injections = Vec::new();
        for (selector, rule) in &raw.injections {
            builder.collect_injections(&mut injections, selector, rule, &raw.repository);
        }
        for injection_scope in lookup.injections(&raw.scope_name) {
            if let Some(grammar) = builder.external_grammar(&injection_scope, None)
                && let Some(selector) = &grammar.injection_selector
            {
                builder.collect_injections(&mut injections, selector, &grammar.as_rule, &grammar.repository);
            }
        }
        injections.sort_by_key(|injection| injection.priority);
        Grammar {
            rules: builder.rules,
            root_id,
            injections,
            language: configuration.language.as_deref().map(Rc::from),
            embedded_languages,
            token_types,
            balanced_brackets: matchers(&configuration.balanced_bracket_selectors),
            unbalanced_brackets: matchers(&configuration.unbalanced_bracket_selectors),
            balanced_allow_any,
            next_line_id: Cell::new(0),
        }
    }

    fn rule(&self, id: usize) -> &Rule {
        self.rules[id].as_ref().expect("rule ids are registered once the grammar is built")
    }

    /// `tokenizeLine`: the tokens of `line` (without its line break) and the state for the next line.
    pub fn tokenize_line(&self, line: &str, previous: Option<&State>) -> Result<(Vec<Token>, State), onig::Error> {
        let (state, is_first_line) = match previous {
            Some(state) => {
                state.reset();
                (state.clone(), false)
            }
            None => (self.initial_state(), true),
        };
        let text = format!("{line}\n");
        let mut tokens = LineTokens { tokens: Vec::new(), last_end: 0 };
        let mut line = Line { text: &text, id: self.new_line_id() };
        let state = self.tokenize_string(&mut line, is_first_line, 0, state, &mut tokens, true)?;
        let line_length = text.len();
        if tokens.tokens.last().is_some_and(|token| token.start == line_length - 1) {
            tokens.tokens.pop();
        }
        if tokens.tokens.is_empty() {
            tokens.last_end = 0;
            tokens.produce(self, &state.content_name_scopes, line_length);
        }
        for token in &mut tokens.tokens {
            token.end = token.end.min(line_length - 1);
        }
        Ok((tokens.tokens, state))
    }

    fn new_line_id(&self) -> u64 {
        self.next_line_id.set(self.next_line_id.get() + 1);
        self.next_line_id.get()
    }

    fn initial_state(&self) -> State {
        let root_name = self.rule(self.root_id).name(None, None).unwrap_or_else(|| "unknown".into());
        let root = Rc::new(ScopeStack {
            parent: None,
            language: self.scope_language(&root_name).or_else(|| self.language.clone()),
            token_type: standard_token_type(&root_name),
            scope_name: root_name,
        });
        Rc::new(StateStack { parent: None, rule_id: self.root_id, enter_pos: Cell::new(-1), anchor_pos: Cell::new(-1), begin_rule_captured_eol: false, end_rule: None, name_scopes: root.clone(), content_name_scopes: root })
    }

    fn scope_language(&self, scope: &str) -> Option<Rc<str>> {
        self.embedded_languages.iter().find(|(prefix, _)| scope == prefix || (scope.starts_with(prefix.as_str()) && scope.as_bytes()[prefix.len()] == b'.')).map(|(_, language)| language.clone())
    }

    /// `AttributedScopeStack.pushAttributed`: a space-separated name pushes one scope per segment.
    fn push_scopes(&self, target: &Rc<ScopeStack>, names: Option<String>) -> Rc<ScopeStack> {
        let Some(names) = names else { return target.clone() };
        let mut result = target.clone();
        for name in names.split(' ') {
            result = Rc::new(ScopeStack {
                language: self.scope_language(name).or_else(|| result.language.clone()),
                token_type: standard_token_type(name).or(result.token_type),
                parent: Some(result),
                scope_name: name.to_string(),
            });
        }
        result
    }

    fn tokenize_string(&self, line: &mut Line, mut is_first_line: bool, mut line_pos: usize, mut stack: State, tokens: &mut LineTokens, check_while_conditions: bool) -> Result<State, onig::Error> {
        let line_length = line.text.len();
        let mut anchor_position: isize = -1;
        if check_while_conditions {
            let result = self.check_while_conditions(line, is_first_line, line_pos, stack, tokens)?;
            stack = result.0;
            line_pos = result.1;
            is_first_line = result.2;
            anchor_position = result.3;
        }
        loop {
            let Some((rule_id, captures)) = self.match_rule_or_injections(line, is_first_line, line_pos, &stack, anchor_position)? else {
                tokens.produce(self, &stack.content_name_scopes, line_length);
                return Ok(stack);
            };
            let (match_start, match_end) = captures[0];
            let has_advanced = match_end > line_pos;

            if rule_id == END_RULE_ID {
                let RuleKind::BeginEnd { end_captures, .. } = &self.rule(stack.rule_id).kind else { unreachable!("end matched outside a begin/end rule") };
                tokens.produce(self, &stack.content_name_scopes, match_start);
                stack = stack.with_content_name_scopes(stack.name_scopes.clone());
                self.handle_captures(line, is_first_line, &stack, tokens, end_captures, &captures)?;
                tokens.produce(self, &stack.content_name_scopes, match_end);
                let popped = stack.clone();
                stack = popped.parent.clone().unwrap();
                anchor_position = popped.anchor_pos.get();
                if !has_advanced && popped.enter_pos.get() == line_pos as isize {
                    // vscode-textmate issue 12: a rule pushed and popped without advancing keeps its state.
                    tokens.produce(self, &popped.content_name_scopes, line_length);
                    return Ok(popped);
                }
            } else {
                let rule_id = rule_id as usize;
                let rule = self.rule(rule_id);
                tokens.produce(self, &stack.content_name_scopes, match_start);
                let before_push = stack.clone();
                let name_scopes = self.push_scopes(&stack.content_name_scopes, rule.name(Some(line.text), Some(&captures)));
                stack = stack.push(rule_id, line_pos as isize, anchor_position, match_end == line_length, None, name_scopes.clone(), name_scopes.clone());
                match &rule.kind {
                    RuleKind::BeginEnd { begin_captures, end, .. } | RuleKind::BeginWhile { begin_captures, while_: end, .. } => {
                        self.handle_captures(line, is_first_line, &stack, tokens, begin_captures, &captures)?;
                        tokens.produce(self, &stack.content_name_scopes, match_end);
                        anchor_position = match_end as isize;
                        let content_scopes = self.push_scopes(&name_scopes, rule.content_name(line.text, &captures));
                        stack = stack.with_content_name_scopes(content_scopes);
                        if end.has_back_references {
                            stack = stack.with_end_rule(end.resolve_back_references(line.text, &captures));
                        }
                        if !has_advanced && before_push.has_same_rule_as(&stack) {
                            stack = stack.parent.clone().unwrap();
                            tokens.produce(self, &stack.content_name_scopes, line_length);
                            return Ok(stack);
                        }
                    }
                    RuleKind::Match { captures: match_captures, .. } => {
                        self.handle_captures(line, is_first_line, &stack, tokens, match_captures, &captures)?;
                        tokens.produce(self, &stack.content_name_scopes, match_end);
                        stack = stack.parent.clone().unwrap();
                        if !has_advanced {
                            stack = stack.safe_pop();
                            tokens.produce(self, &stack.content_name_scopes, line_length);
                            return Ok(stack);
                        }
                    }
                    RuleKind::Capture { .. } | RuleKind::IncludeOnly { .. } => unreachable!("scanners only hold match, begin and end patterns"),
                }
            }
            if match_end > line_pos {
                line_pos = match_end;
                is_first_line = false;
            }
        }
    }

    /// Walks the stack bottom to top; the first `while` that fails cuts the stack above it.
    fn check_while_conditions(&self, line: &mut Line, mut is_first_line: bool, mut line_pos: usize, mut stack: State, tokens: &mut LineTokens) -> Result<(State, usize, bool, isize), onig::Error> {
        let mut anchor_position: isize = if stack.begin_rule_captured_eol { 0 } else { -1 };
        let mut while_rules = Vec::new();
        let mut node = Some(stack.clone());
        while let Some(current) = node {
            if matches!(self.rule(current.rule_id).kind, RuleKind::BeginWhile { .. }) {
                while_rules.push(current.clone());
            }
            node = current.parent.clone();
        }
        while let Some(while_stack) = while_rules.pop() {
            let scanner = self.compile_while(while_stack.rule_id, while_stack.end_rule.as_deref(), is_first_line, line_pos as isize == anchor_position)?;
            match scanner.find_next_match(line.text, line.id, line_pos) {
                Some((rule_id, captures)) if rule_id == WHILE_RULE_ID => {
                    let RuleKind::BeginWhile { while_captures, .. } = &self.rule(while_stack.rule_id).kind else { unreachable!() };
                    tokens.produce(self, &while_stack.content_name_scopes, captures[0].0);
                    self.handle_captures(line, is_first_line, &while_stack, tokens, while_captures, &captures)?;
                    tokens.produce(self, &while_stack.content_name_scopes, captures[0].1);
                    anchor_position = captures[0].1 as isize;
                    if captures[0].1 > line_pos {
                        line_pos = captures[0].1;
                        is_first_line = false;
                    }
                }
                _ => {
                    stack = while_stack.parent.clone().unwrap();
                    break;
                }
            }
        }
        Ok((stack, line_pos, is_first_line, anchor_position))
    }

    fn match_rule_or_injections(&self, line: &Line, is_first_line: bool, line_pos: usize, stack: &State, anchor_position: isize) -> Result<Option<(i64, Captures)>, onig::Error> {
        let allow_g = line_pos as isize == anchor_position;
        let scanner = self.compile_ag(stack.rule_id, stack.end_rule.as_deref(), is_first_line, allow_g)?;
        let match_result = scanner.find_next_match(line.text, line.id, line_pos);
        if self.injections.is_empty() {
            return Ok(match_result);
        }
        let scopes = stack.content_name_scopes.names();
        let mut best: Option<(i64, Captures, i32)> = None;
        for injection in &self.injections {
            if !injection.matcher.matches(&scopes) {
                continue;
            }
            let scanner = self.compile_ag(injection.rule_id, None, is_first_line, allow_g)?;
            let Some((rule_id, captures)) = scanner.find_next_match(line.text, line.id, line_pos) else { continue };
            // Injections are sorted by priority, so an earlier one at the same position wins.
            if best.as_ref().is_some_and(|(_, best, _)| captures[0].0 >= best[0].0) {
                continue;
            }
            let at_start = captures[0].0 == line_pos;
            best = Some((rule_id, captures, injection.priority));
            if at_start {
                break;
            }
        }
        let Some((injection_rule, injection_captures, priority)) = best else { return Ok(match_result) };
        let Some((rule_id, captures)) = match_result else { return Ok(Some((injection_rule, injection_captures))) };
        let (injection_start, match_start) = (injection_captures[0].0, captures[0].0);
        if injection_start < match_start || (priority == -1 && injection_start == match_start) {
            return Ok(Some((injection_rule, injection_captures)));
        }
        Ok(Some((rule_id, captures)))
    }

    /// `rule.compileAG`: the scanner over a rule's patterns, its `end` placed first or last.
    fn compile_ag(&self, rule_id: usize, end_source: Option<&str>, allow_a: bool, allow_g: bool) -> Result<Rc<Scanner>, onig::Error> {
        let rule = self.rule(rule_id);
        let mut cached = rule.cached.borrow_mut();
        let list = cached.get_or_insert_with(|| {
            let mut list = RegExpSourceList::default();
            match &rule.kind {
                RuleKind::Match { regex, .. } => list.push(regex.clone()),
                RuleKind::IncludeOnly { patterns } => self.collect_patterns(patterns, &mut list),
                RuleKind::BeginEnd { end, apply_end_pattern_last, patterns, .. } => {
                    self.collect_patterns(patterns, &mut list);
                    if *apply_end_pattern_last {
                        list.push(end.clone());
                    } else {
                        list.unshift(end.clone());
                    }
                }
                RuleKind::BeginWhile { patterns, .. } => self.collect_patterns(patterns, &mut list),
                RuleKind::Capture { .. } => unreachable!("capture rules are never scanned"),
            }
            list
        });
        if let RuleKind::BeginEnd { end, apply_end_pattern_last, .. } = &rule.kind
            && end.has_back_references
        {
            let index = if *apply_end_pattern_last { list.items.len() - 1 } else { 0 };
            list.set_source(index, end_source.unwrap_or(&end.source));
        }
        list.compile_ag(allow_a, allow_g)
    }

    fn compile_while(&self, rule_id: usize, end_source: Option<&str>, allow_a: bool, allow_g: bool) -> Result<Rc<Scanner>, onig::Error> {
        let rule = self.rule(rule_id);
        let RuleKind::BeginWhile { while_, .. } = &rule.kind else { unreachable!() };
        let mut cached = rule.cached_while.borrow_mut();
        let list = cached.get_or_insert_with(|| {
            let mut list = RegExpSourceList::default();
            list.push(while_.clone());
            list
        });
        if while_.has_back_references {
            list.set_source(0, end_source.unwrap_or("\u{FFFF}"));
        }
        list.compile_ag(allow_a, allow_g)
    }

    /// `collectPatterns`: a match pattern, a begin pattern, or an include's patterns, recursively.
    fn collect_patterns(&self, patterns: &[usize], out: &mut RegExpSourceList) {
        for &id in patterns {
            match &self.rule(id).kind {
                RuleKind::Match { regex, .. } => out.push(regex.clone()),
                RuleKind::BeginEnd { begin, .. } | RuleKind::BeginWhile { begin, .. } => out.push(begin.clone()),
                RuleKind::IncludeOnly { patterns } => self.collect_patterns(patterns, out),
                RuleKind::Capture { .. } => unreachable!("capture rules are not patterns"),
            }
        }
    }

    fn handle_captures(&self, line: &mut Line, is_first_line: bool, stack: &State, tokens: &mut LineTokens, captures: &[Option<usize>], indices: &Captures) -> Result<(), onig::Error> {
        let max_end = indices[0].1;
        let mut local_stack: Vec<(Rc<ScopeStack>, usize)> = Vec::new();
        for (i, capture_rule) in captures.iter().take(indices.len()).enumerate() {
            let Some(capture_rule_id) = *capture_rule else { continue };
            let (start, end) = indices[i];
            if start == end || start == usize::MAX {
                continue;
            }
            if start > max_end {
                break;
            }
            while let Some((scopes, end_pos)) = local_stack.last().filter(|(_, end_pos)| *end_pos <= start) {
                tokens.produce(self, &scopes.clone(), *end_pos);
                local_stack.pop();
            }
            match local_stack.last() {
                Some((scopes, _)) => tokens.produce(self, &scopes.clone(), start),
                None => tokens.produce(self, &stack.content_name_scopes, start),
            }
            let capture_rule = self.rule(capture_rule_id);
            let RuleKind::Capture { retokenize_with } = capture_rule.kind else { unreachable!() };
            if let Some(retokenize_rule) = retokenize_with {
                let name_scopes = self.push_scopes(&stack.content_name_scopes, capture_rule.name(Some(line.text), Some(indices)));
                let content_scopes = self.push_scopes(&name_scopes, capture_rule.content_name(line.text, indices));
                let stack_clone = stack.push(retokenize_rule, start as isize, -1, false, None, name_scopes, content_scopes);
                let mut sub_line = Line { text: &line.text[..end], id: self.new_line_id() };
                self.tokenize_string(&mut sub_line, is_first_line && start == 0, start, stack_clone, tokens, false)?;
                continue;
            }
            if let Some(name) = capture_rule.name(Some(line.text), Some(indices)) {
                let base = local_stack.last().map_or(&stack.content_name_scopes, |(scopes, _)| scopes);
                let scopes = self.push_scopes(base, Some(name));
                local_stack.push((scopes, end));
            }
        }
        while let Some((scopes, end_pos)) = local_stack.pop() {
            tokens.produce(self, &scopes, end_pos);
        }
        Ok(())
    }

    fn token_type_and_brackets(&self, scopes: &Rc<ScopeStack>) -> (StandardTokenType, bool) {
        let mut token_type = scopes.token_type.unwrap_or(StandardTokenType::Other);
        let matches_always = self.balanced_allow_any && self.unbalanced_brackets.is_empty();
        let matches_never = self.balanced_brackets.is_empty() && !self.balanced_allow_any;
        if self.token_types.is_empty() && (matches_always || matches_never) {
            return (token_type, matches_always);
        }
        let names = scopes.names();
        for (matcher, override_type) in &self.token_types {
            if matcher.matches(&names) {
                token_type = *override_type;
            }
        }
        let balanced = if matches_always {
            true
        } else if self.unbalanced_brackets.iter().any(|m| m.matches(&names)) {
            false
        } else {
            self.balanced_brackets.iter().any(|m| m.matches(&names)) || self.balanced_allow_any
        };
        (token_type, balanced)
    }
}

/// The text being scanned; `id` is fresh per text so scanners can cache searches on it.
struct Line<'a> {
    text: &'a str,
    id: u64,
}

struct LineTokens {
    tokens: Vec<Token>,
    last_end: usize,
}

impl LineTokens {
    fn produce(&mut self, grammar: &Grammar, scopes: &Rc<ScopeStack>, end: usize) {
        if self.last_end >= end {
            return;
        }
        let (token_type, balanced_brackets) = grammar.token_type_and_brackets(scopes);
        self.tokens.push(Token { start: self.last_end, end, scopes: scopes.clone(), token_type, balanced_brackets });
        self.last_end = end;
    }
}

/// `STANDARD_TOKEN_TYPE_REGEXP`: the first of `comment`, `string`, `regex`, `meta.embedded` that stands as whole words.
fn standard_token_type(scope: &str) -> Option<StandardTokenType> {
    let bytes = scope.as_bytes();
    let is_word = |i: usize| bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    for start in 0..bytes.len() {
        if start > 0 && is_word(start - 1) {
            continue;
        }
        for (word, token_type) in [("comment", Some(StandardTokenType::Comment)), ("string", Some(StandardTokenType::String)), ("regex", Some(StandardTokenType::RegEx)), ("meta.embedded", Some(StandardTokenType::Other))] {
            if scope[start..].starts_with(word) && !is_word(start + word.len()) {
                return token_type;
            }
        }
    }
    None
}

/// `RuleFactory` with the grammar's rule registry and its included grammars.
struct Builder<'a> {
    rules: Vec<Option<Rule>>,
    included: HashMap<String, Rc<RawGrammar>>,
    lookup: &'a dyn GrammarLookup,
}

impl Builder<'_> {
    fn external_grammar(&mut self, scope_name: &str, repository: Option<&Repository>) -> Option<Rc<RawGrammar>> {
        if let Some(grammar) = self.included.get(scope_name) {
            return Some(grammar.clone());
        }
        let base = repository.map(|repository| repository["$base"].clone());
        let grammar = Rc::new(self.lookup.raw_grammar(scope_name, base)?);
        self.included.insert(scope_name.to_string(), grammar.clone());
        Some(grammar)
    }

    fn collect_injections(&mut self, result: &mut Vec<Injection>, selector: &str, rule: &Rc<RawRule>, repository: &Repository) {
        let rule_id = self.compiled_rule_id(rule, repository);
        for matcher in create_matchers(selector) {
            result.push(Injection { matcher: matcher.matcher, priority: matcher.priority, rule_id });
        }
    }

    fn compiled_rule_id(&mut self, desc: &Rc<RawRule>, repository: &Repository) -> usize {
        if let Some(id) = desc.id.get() {
            return id;
        }
        let id = self.rules.len();
        self.rules.push(None);
        desc.id.set(Some(id));
        let rule = self.create_rule(id, desc, repository);
        self.rules[id] = Some(rule);
        id
    }

    fn create_rule(&mut self, id: usize, desc: &Rc<RawRule>, repository: &Repository) -> Rule {
        let id = id as i64;
        if let Some(regex) = desc.match_.as_deref().filter(|m| !m.is_empty()) {
            let captures = self.compile_captures(desc.captures.as_ref(), repository);
            return Rule::new(desc.name.clone(), None, RuleKind::Match { regex: RegExpSource::new(regex, id), captures });
        }
        let Some(begin) = desc.begin.as_deref() else {
            let merged;
            let repository = match &desc.repository {
                Some(own) => {
                    let mut map = (**repository).clone();
                    map.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
                    merged = Rc::new(map);
                    &merged
                }
                None => repository,
            };
            let include_only;
            let patterns = match (&desc.patterns, desc.include.as_ref().filter(|i| !i.is_empty())) {
                (None, Some(include)) => {
                    include_only = vec![Rc::new(RawRule { include: Some(include.clone()), ..Default::default() })];
                    Some(&include_only)
                }
                (patterns, _) => patterns.as_ref(),
            };
            let (patterns, missing) = self.compile_patterns(patterns.map(Vec::as_slice), repository);
            let mut rule = Rule::new(desc.name.clone(), desc.content_name.clone(), RuleKind::IncludeOnly { patterns });
            rule.has_missing_patterns = missing;
            return rule;
        };
        if let Some(while_) = desc.while_.as_deref().filter(|w| !w.is_empty()) {
            let begin_captures = self.compile_captures(desc.begin_captures.as_ref().or(desc.captures.as_ref()), repository);
            let while_captures = self.compile_captures(desc.while_captures.as_ref().or(desc.captures.as_ref()), repository);
            let (patterns, missing) = self.compile_patterns(desc.patterns.as_deref(), repository);
            let mut rule = Rule::new(desc.name.clone(), desc.content_name.clone(), RuleKind::BeginWhile {
                begin: RegExpSource::new(begin, id),
                begin_captures,
                while_: RegExpSource::new(while_, WHILE_RULE_ID),
                while_captures,
                patterns,
            });
            rule.has_missing_patterns = missing;
            return rule;
        }
        let begin_captures = self.compile_captures(desc.begin_captures.as_ref().or(desc.captures.as_ref()), repository);
        let end_captures = self.compile_captures(desc.end_captures.as_ref().or(desc.captures.as_ref()), repository);
        let (patterns, missing) = self.compile_patterns(desc.patterns.as_deref(), repository);
        let end = desc.end.as_deref().filter(|e| !e.is_empty()).unwrap_or("\u{FFFF}");
        let mut rule = Rule::new(desc.name.clone(), desc.content_name.clone(), RuleKind::BeginEnd {
            begin: RegExpSource::new(begin, id),
            begin_captures,
            end: RegExpSource::new(end, END_RULE_ID),
            end_captures,
            apply_end_pattern_last: desc.apply_end_pattern_last,
            patterns,
        });
        rule.has_missing_patterns = missing;
        rule
    }

    fn compile_captures(&mut self, captures: Option<&RawCaptures>, repository: &Repository) -> Vec<Option<usize>> {
        let Some(captures) = captures else { return Vec::new() };
        let max = captures.iter().map(|(index, _)| *index).max().unwrap_or(0);
        let mut result = vec![None; max + 1];
        for (index, capture) in captures {
            let retokenize_with = capture.patterns.is_some().then(|| self.compiled_rule_id(capture, repository));
            let id = self.rules.len();
            self.rules.push(Some(Rule::new(capture.name.clone(), capture.content_name.clone(), RuleKind::Capture { retokenize_with })));
            result[*index] = Some(id);
        }
        result
    }

    /// The compiled pattern ids, and whether any pattern was lost.
    fn compile_patterns(&mut self, patterns: Option<&[Rc<RawRule>]>, repository: &Repository) -> (Vec<usize>, bool) {
        let Some(patterns) = patterns else { return (Vec::new(), false) };
        let mut result = Vec::new();
        for pattern in patterns {
            let rule_id = match pattern.include.as_deref().filter(|i| !i.is_empty()) {
                Some(include) => self.include_rule_id(include, repository),
                None => Some(self.compiled_rule_id(pattern, repository)),
            };
            let Some(rule_id) = rule_id else { continue };
            // A rule whose patterns all went missing is dropped; one still being built is kept.
            if let Some(rule) = &self.rules[rule_id]
                && rule.patterns().is_some_and(|patterns| patterns.is_empty())
                && rule.has_missing_patterns
            {
                continue;
            }
            result.push(rule_id);
        }
        let missing = result.len() != patterns.len();
        (result, missing)
    }

    fn include_rule_id(&mut self, include: &str, repository: &Repository) -> Option<usize> {
        if include == "$base" || include == "$self" {
            let rule = repository.get(include)?.clone();
            return Some(self.compiled_rule_id(&rule, repository));
        }
        let (scope_name, rule_name) = match include.find('#') {
            Some(0) => {
                let rule = repository.get(&include[1..])?.clone();
                return Some(self.compiled_rule_id(&rule, repository));
            }
            Some(i) => (&include[..i], Some(&include[i + 1..])),
            None => (include, None),
        };
        let grammar = self.external_grammar(scope_name, Some(repository))?;
        let rule = grammar.repository.get(rule_name.unwrap_or("$self"))?.clone();
        Some(self.compiled_rule_id(&rule, &grammar.repository))
    }
}
