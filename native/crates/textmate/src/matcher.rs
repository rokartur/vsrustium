//! Scope selectors as vscode-textmate's matcher.ts parses them: `a b`, `-a`, `a | b`, `(a, b)`,
//! and the `L:`/`R:` injection priorities.

pub enum Matcher {
    Names(Vec<String>),
    /// `-x`; a `-` with nothing after it matches nothing.
    Not(Option<Box<Matcher>>),
    And(Vec<Matcher>),
    Or(Vec<Matcher>),
}

pub struct MatcherWithPriority {
    pub matcher: Matcher,
    /// -1 for `L:`, 1 for `R:`, else 0.
    pub priority: i32,
}

pub fn create_matchers(selector: &str) -> Vec<MatcherWithPriority> {
    let mut parser = Parser { tokens: tokenize(selector), index: 0 };
    let mut results = Vec::new();
    while let Some(token) = parser.peek() {
        let mut priority = 0;
        if token.len() == 2 && token.ends_with(':') {
            priority = match token.as_bytes()[0] {
                b'R' => 1,
                b'L' => -1,
                _ => 0,
            };
            parser.index += 1;
        }
        results.push(MatcherWithPriority { matcher: parser.conjunction(), priority });
        if parser.peek() != Some(",") {
            break;
        }
        parser.index += 1;
    }
    results
}

impl Matcher {
    pub fn matches(&self, scopes: &[&str]) -> bool {
        match self {
            Matcher::Names(names) => names_match(names, scopes),
            Matcher::Not(inner) => inner.as_ref().is_some_and(|inner| !inner.matches(scopes)),
            Matcher::And(all) => all.iter().all(|m| m.matches(scopes)),
            Matcher::Or(any) => any.iter().any(|m| m.matches(scopes)),
        }
    }
}

/// grammar.ts `nameMatcher`: every identifier matches a scope, in order.
fn names_match(identifiers: &[String], scopes: &[&str]) -> bool {
    if scopes.len() < identifiers.len() {
        return false;
    }
    let mut last_index = 0;
    identifiers.iter().all(|identifier| {
        for (i, scope) in scopes.iter().enumerate().skip(last_index) {
            if scopes_are_matching(scope, identifier) {
                last_index = i + 1;
                return true;
            }
        }
        false
    })
}

fn scopes_are_matching(this_scope_name: &str, scope_name: &str) -> bool {
    if this_scope_name.is_empty() {
        return false;
    }
    this_scope_name == scope_name
        || (this_scope_name.len() > scope_name.len() && this_scope_name.starts_with(scope_name) && this_scope_name.as_bytes()[scope_name.len()] == b'.')
}

struct Parser {
    tokens: Vec<String>,
    index: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    fn operand(&mut self) -> Option<Matcher> {
        let token = self.peek()?;
        if token == "-" {
            self.index += 1;
            return Some(Matcher::Not(self.operand().map(Box::new)));
        }
        if token == "(" {
            self.index += 1;
            let inner = self.inner_expression();
            if self.peek() == Some(")") {
                self.index += 1;
            }
            return Some(inner);
        }
        if !is_identifier(token) {
            return None;
        }
        let mut identifiers = Vec::new();
        while let Some(token) = self.peek().filter(|token| is_identifier(token)) {
            identifiers.push(token.to_string());
            self.index += 1;
        }
        Some(Matcher::Names(identifiers))
    }

    fn conjunction(&mut self) -> Matcher {
        let mut matchers = Vec::new();
        while let Some(matcher) = self.operand() {
            matchers.push(matcher);
        }
        Matcher::And(matchers)
    }

    fn inner_expression(&mut self) -> Matcher {
        let mut matchers = vec![self.conjunction()];
        while matches!(self.peek(), Some("|" | ",")) {
            while matches!(self.peek(), Some("|" | ",")) {
                self.index += 1;
            }
            matchers.push(self.conjunction());
        }
        Matcher::Or(matchers)
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_identifier(token: &str) -> bool {
    token.chars().any(|c| is_word(c) || c == '.' || c == ':')
}

/// The token regex `([LR]:|[\w\.:][\w\.:\-]*|[\,\|\-\(\)])`, skipping anything else.
fn tokenize(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (c == 'L' || c == 'R') && chars.get(i + 1) == Some(&':') {
            tokens.push(format!("{c}:"));
            i += 2;
        } else if is_word(c) || c == '.' || c == ':' {
            let start = i;
            while i < chars.len() && (is_word(chars[i]) || matches!(chars[i], '.' | ':' | '-')) {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else {
            if matches!(c, ',' | '|' | '-' | '(' | ')') {
                tokens.push(c.to_string());
            }
            i += 1;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(selector: &str, scopes: &[&str]) -> bool {
        create_matchers(selector)[0].matcher.matches(scopes)
    }

    #[test]
    fn matches_like_vscode_textmate() {
        assert!(matches("foo", &["foo"]));
        assert!(matches("foo", &["foo.bar"]));
        assert!(!matches("foo", &["foobar"]));
        assert!(matches("foo bar", &["foo", "x", "bar.baz"]));
        assert!(!matches("foo -bar", &["foo", "bar"]));
        assert!(matches("foo -bar", &["foo", "baz"]));
        assert!(matches("(foo, baz) bar", &["baz", "bar"]));
        assert!(matches("(foo | bar)", &["bar"]));
        // A top-level `|` ends the selector, as in vscode-textmate.
        assert!(!matches("foo | bar", &["bar"]));
        let injections = create_matchers("L:text.html.markdown - (comment | string), R:source.js");
        assert_eq!(injections.iter().map(|m| m.priority).collect::<Vec<_>>(), [-1, 1]);
        assert!(!injections[0].matcher.matches(&["text.html.markdown", "comment.block"]));
    }
}
