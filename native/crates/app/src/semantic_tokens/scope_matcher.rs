//! The scope selectors of theme rules as `ColorThemeData.resolveScopes` scores them: `createMatchers` of
//! workbench/services/themes/common/textMateScopeMatcher.ts with colorThemeData.ts `nameMatcher`.

pub enum ScopeMatcher {
    Names(Vec<String>),
    Not(Box<ScopeMatcher>),
    And(Vec<ScopeMatcher>),
    Or(Vec<ScopeMatcher>),
}

/// `createMatchers`: the `L:`/`R:` priorities are skipped, as `getScopeMatcher` ignores them.
pub fn create_matchers(selector: &str, results: &mut Vec<ScopeMatcher>) {
    let mut parser = Parser { tokens: tokenize(selector), index: 0 };
    while let Some(token) = parser.peek() {
        if token.len() == 2 && token.ends_with(':') {
            parser.index += 1;
        }
        if let Some(matcher) = parser.conjunction() {
            results.push(matcher);
        }
        if parser.peek() != Some(",") {
            break;
        }
        parser.index += 1;
    }
}

impl ScopeMatcher {
    /// The match score of a probe scope (outermost scope first), -1 when it does not match.
    pub fn score(&self, scopes: &[String]) -> i64 {
        match self {
            ScopeMatcher::Names(identifiers) => name_score(identifiers, scopes),
            ScopeMatcher::Not(inner) => {
                if inner.score(scopes) < 0 { 0 } else { -1 }
            }
            ScopeMatcher::And(all) => {
                let mut min = all[0].score(scopes);
                for matcher in &all[1..] {
                    if min < 0 {
                        break;
                    }
                    min = min.min(matcher.score(scopes));
                }
                min
            }
            ScopeMatcher::Or(any) => any.iter().map(|matcher| matcher.score(scopes)).max().unwrap(),
        }
    }
}

/// colorThemeData.ts `nameMatcher`: every identifier matches some scope; the score is the last identifier's
/// innermost match.
fn name_score(identifiers: &[String], scopes: &[String]) -> i64 {
    if scopes.len() < identifiers.len() {
        return -1;
    }
    let mut score = -1;
    for identifier in identifiers {
        let Some(index) = scopes.iter().rposition(|scope| scopes_are_matching(scope, identifier)) else { return -1 };
        score = (index as i64 + 1) * 0x10000 + identifier.encode_utf16().count() as i64;
    }
    score
}

/// `scopesAreMatching`: `scope` is `selector` or a dotted child of it.
fn scopes_are_matching(scope: &str, selector: &str) -> bool {
    if scope.is_empty() {
        return false;
    }
    scope == selector || (scope.len() > selector.len() && scope.starts_with(selector) && scope.as_bytes()[selector.len()] == b'.')
}

struct Parser {
    tokens: Vec<String>,
    index: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    fn operand(&mut self) -> Option<ScopeMatcher> {
        let token = self.peek()?.to_owned();
        if token == "-" {
            self.index += 1;
            return Some(ScopeMatcher::Not(Box::new(self.operand()?)));
        }
        if token == "(" {
            self.index += 1;
            let inner = self.inner_expression();
            if self.peek() == Some(")") {
                self.index += 1;
            }
            return inner;
        }
        if !is_identifier(&token) {
            return None;
        }
        let mut identifiers = Vec::new();
        while let Some(identifier) = self.peek().filter(|token| is_identifier(token)) {
            identifiers.push(identifier.to_owned());
            self.index += 1;
        }
        Some(ScopeMatcher::Names(identifiers))
    }

    fn conjunction(&mut self) -> Option<ScopeMatcher> {
        let mut all = vec![self.operand()?];
        while let Some(matcher) = self.operand() {
            all.push(matcher);
        }
        Some(ScopeMatcher::And(all))
    }

    fn inner_expression(&mut self) -> Option<ScopeMatcher> {
        let mut any = vec![self.conjunction()?];
        while matches!(self.peek(), Some("|" | ",")) {
            while matches!(self.peek(), Some("|" | ",")) {
                self.index += 1;
            }
            match self.conjunction() {
                Some(matcher) => any.push(matcher),
                None => break,
            }
        }
        Some(ScopeMatcher::Or(any))
    }
}

/// JS `\w` without the `u` flag, plus the `.` and `:` the selector regexes add.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ':'
}

/// `isIdentifier`: `/[\w\.:]+/` anywhere in the token.
fn is_identifier(token: &str) -> bool {
    token.chars().any(is_name_char)
}

/// `newTokenizer`: `/([LR]:|[\w\.:][\w\.:\-]*|[\,\|\-\(\)])/g`, other characters skipped.
fn tokenize(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (c == 'L' || c == 'R') && chars.get(i + 1) == Some(&':') {
            tokens.push(format!("{c}:"));
            i += 2;
        } else if is_name_char(c) {
            let start = i;
            i += 1;
            while i < chars.len() && (is_name_char(chars[i]) || chars[i] == '-') {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else {
            if ",|-()".contains(c) {
                tokens.push(c.to_string());
            }
            i += 1;
        }
    }
    tokens
}
