//! Context keys and `when` clauses: platform/contextkey/common/scanner.ts and contextkey.ts.
//! Expressions normalize, serialize and evaluate as upstream's; values follow JavaScript's loose rules.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use onig::{Regex, RegexOptions, Syntax};
use serde_json::Value;

/// A lookup of context key values (`IContext`); `None` is JavaScript's `undefined`.
pub trait Context {
    fn value(&self, key: &str) -> Option<&Value>;
}

impl Context for HashMap<String, Value> {
    fn value(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }
}

/// `CONSTANT_VALUES` on the native macOS build.
fn constant(key: &str) -> Option<bool> {
    match key {
        "true" | "isMac" | "isMacNative" => Some(true),
        "false" | "isLinux" | "isWindows" | "isChromeOS" | "isWeb" | "isEdge" | "isFirefox" | "isChrome" | "isSafari" => Some(false),
        _ => None,
    }
}

/// The right side of `<`, `<=`, `>`, `>=` after `withFloatOrStr`.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Number(f64),
    String(String),
}

/// A JavaScript `RegExp`: its `source` (slashes escaped) and canonical `flags`.
#[derive(Clone)]
pub struct JsRegex {
    source: String,
    flags: String,
    compiled: Arc<Regex>,
}

impl std::fmt::Debug for JsRegex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "/{}/{}", self.source, self.flags)
    }
}

impl JsRegex {
    /// `new RegExp(pattern, flags)`; `None` where JavaScript throws.
    pub fn new(pattern: &str, flags: &str) -> Option<JsRegex> {
        let mut canonical = String::new();
        for flag in "dgimsuvy".chars() {
            match flags.matches(flag).count() {
                0 => {}
                1 => canonical.push(flag),
                _ => return None,
            }
        }
        if flags.chars().any(|flag| !"dgimsuvy".contains(flag)) {
            return None;
        }
        // Perl syntax runs with SINGLELINE: `^` and `$` anchor the whole string unless `m` negates it, as in JavaScript.
        let mut options = RegexOptions::REGEX_OPTION_NONE;
        if canonical.contains('i') {
            options |= RegexOptions::REGEX_OPTION_IGNORECASE;
        }
        if canonical.contains('m') {
            options |= RegexOptions::REGEX_OPTION_NEGATE_SINGLELINE;
        }
        if canonical.contains('s') {
            options |= RegexOptions::REGEX_OPTION_MULTILINE;
        }
        let compiled = Regex::with_options(pattern, options, Syntax::perl_ng()).ok()?;
        Some(JsRegex { source: escape_regex_source(pattern), flags: canonical, compiled: Arc::new(compiled) })
    }

    fn test(&self, value: Option<&Value>) -> bool {
        self.compiled.find(&js_string(value)).is_some()
    }
}

/// `RegExp.prototype.source`: an unescaped `/` outside a character class becomes `\/`, line breaks become escapes.
fn escape_regex_source(pattern: &str) -> String {
    if pattern.is_empty() {
        return "(?:)".into();
    }
    let mut out = String::new();
    let mut in_escape = false;
    let mut in_class = false;
    for ch in pattern.chars() {
        match ch {
            '\n' => out.push_str(if in_escape { "n" } else { "\\n" }),
            '\r' => out.push_str(if in_escape { "r" } else { "\\r" }),
            '/' if !in_escape && !in_class => out.push_str("\\/"),
            _ => out.push(ch),
        }
        if in_escape {
            in_escape = false;
        } else if ch == '\\' {
            in_escape = true;
        } else if ch == '[' {
            in_class = true;
        } else if ch == ']' {
            in_class = false;
        }
    }
    out
}

/// `ContextKeyExpression`; the variant order is `ContextKeyExprType`, which sorting uses.
type Negation = Option<Arc<Expr>>;

#[derive(Clone, Debug)]
pub enum Expr {
    False,
    True,
    Defined(String),
    Not(String),
    Equals(String, Value),
    NotEquals(String, Value),
    /// The second field is the expression this one negates; negating returns it, as upstream caches `negated`.
    And(Vec<Expr>, Negation),
    Regex(String, JsRegex),
    NotRegex(String, JsRegex),
    Or(Vec<Expr>, Negation),
    In(String, String),
    NotIn(String, String),
    Greater(String, Scalar),
    GreaterEquals(String, Scalar),
    Smaller(String, Scalar),
    SmallerEquals(String, Scalar),
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl Expr {
    pub fn has(key: &str) -> Expr {
        match constant(key) {
            Some(value) => Expr::boolean(value),
            None => Expr::Defined(key.into()),
        }
    }

    pub fn not(key: &str) -> Expr {
        match constant(key) {
            Some(value) => Expr::boolean(!value),
            None => Expr::Not(key.into()),
        }
    }

    pub fn key_equals(key: &str, value: Value) -> Expr {
        if let Value::Bool(value) = value {
            return if value { Expr::has(key) } else { Expr::not(key) };
        }
        match constant(key) {
            Some(constant) => Expr::boolean(value == Value::String(constant.to_string())),
            None => Expr::Equals(key.into(), value),
        }
    }

    pub fn key_not_equals(key: &str, value: Value) -> Expr {
        if let Value::Bool(value) = value {
            return if value { Expr::not(key) } else { Expr::has(key) };
        }
        match constant(key) {
            Some(constant) => Expr::boolean(value != Value::String(constant.to_string())),
            None => Expr::NotEquals(key.into(), value),
        }
    }

    pub fn regex(key: &str, regex: JsRegex) -> Expr {
        Expr::Regex(key.into(), regex)
    }

    pub fn is_in(key: &str, value_key: &str) -> Expr {
        Expr::In(key.into(), value_key.into())
    }

    pub fn not_in(key: &str, value_key: &str) -> Expr {
        Expr::NotIn(key.into(), value_key.into())
    }

    pub fn greater(key: &str, value: Value) -> Expr {
        with_float_or_str(value, |value| Expr::Greater(key.into(), value))
    }

    pub fn greater_equals(key: &str, value: Value) -> Expr {
        with_float_or_str(value, |value| Expr::GreaterEquals(key.into(), value))
    }

    pub fn smaller(key: &str, value: Value) -> Expr {
        with_float_or_str(value, |value| Expr::Smaller(key.into(), value))
    }

    pub fn smaller_equals(key: &str, value: Value) -> Expr {
        with_float_or_str(value, |value| Expr::SmallerEquals(key.into(), value))
    }

    /// `ContextKeyExpr.and`: `None` for no terms.
    pub fn and(terms: Vec<Option<Expr>>) -> Option<Expr> {
        and_create(terms, None, true)
    }

    pub fn or(terms: Vec<Option<Expr>>) -> Option<Expr> {
        or_create(terms, None, true)
    }

    /// `ContextKeyExpr.deserialize`: no regex error recovery; `None` on any error.
    pub fn deserialize(input: &str) -> Option<Expr> {
        Parser::new(false).parse(input)
    }

    fn boolean(value: bool) -> Expr {
        if value { Expr::True } else { Expr::False }
    }

    fn type_index(&self) -> i32 {
        match self {
            Expr::False => 0,
            Expr::True => 1,
            Expr::Defined(_) => 2,
            Expr::Not(_) => 3,
            Expr::Equals(..) => 4,
            Expr::NotEquals(..) => 5,
            Expr::And(..) => 6,
            Expr::Regex(..) => 7,
            Expr::NotRegex(..) => 8,
            Expr::Or(..) => 9,
            Expr::In(..) => 10,
            Expr::NotIn(..) => 11,
            Expr::Greater(..) => 12,
            Expr::GreaterEquals(..) => 13,
            Expr::Smaller(..) => 14,
            Expr::SmallerEquals(..) => 15,
        }
    }

    pub fn is_true(&self) -> bool {
        matches!(self, Expr::True)
    }

    pub fn is_false(&self) -> bool {
        matches!(self, Expr::False)
    }

    pub fn cmp(&self, other: &Expr) -> Ordering {
        let by_type = self.type_index().cmp(&other.type_index());
        if by_type != Ordering::Equal {
            return by_type;
        }
        match (self, other) {
            (Expr::Defined(a), Expr::Defined(b)) | (Expr::Not(a), Expr::Not(b)) => a.cmp(b),
            (Expr::Equals(a, x), Expr::Equals(b, y)) | (Expr::NotEquals(a, x), Expr::NotEquals(b, y)) => a.cmp(b).then_with(|| js_compare(&prim(x), &prim(y))),
            (Expr::In(a, x), Expr::In(b, y)) | (Expr::NotIn(a, x), Expr::NotIn(b, y)) => a.cmp(b).then_with(|| x.cmp(y)),
            (Expr::Greater(a, x), Expr::Greater(b, y))
            | (Expr::GreaterEquals(a, x), Expr::GreaterEquals(b, y))
            | (Expr::Smaller(a, x), Expr::Smaller(b, y))
            | (Expr::SmallerEquals(a, x), Expr::SmallerEquals(b, y)) => a.cmp(b).then_with(|| js_compare(&scalar_prim(x), &scalar_prim(y))),
            (Expr::Regex(a, x), Expr::Regex(b, y)) | (Expr::NotRegex(a, x), Expr::NotRegex(b, y)) => a.cmp(b).then_with(|| x.source.cmp(&y.source)),
            (Expr::And(a, _), Expr::And(b, _)) | (Expr::Or(a, _), Expr::Or(b, _)) => {
                let by_length = a.len().cmp(&b.len());
                if by_length != Ordering::Equal {
                    return by_length;
                }
                a.iter().zip(b).map(|(x, y)| x.cmp(y)).find(|order| *order != Ordering::Equal).unwrap_or(Ordering::Equal)
            }
            _ => Ordering::Equal,
        }
    }

    pub fn equals(&self, other: &Expr) -> bool {
        match (self, other) {
            (Expr::False, Expr::False) | (Expr::True, Expr::True) => true,
            (Expr::Defined(a), Expr::Defined(b)) | (Expr::Not(a), Expr::Not(b)) => a == b,
            (Expr::Equals(a, x), Expr::Equals(b, y)) | (Expr::NotEquals(a, x), Expr::NotEquals(b, y)) => a == b && x == y,
            (Expr::In(a, x), Expr::In(b, y)) | (Expr::NotIn(a, x), Expr::NotIn(b, y)) => a == b && x == y,
            (Expr::Greater(a, x), Expr::Greater(b, y))
            | (Expr::GreaterEquals(a, x), Expr::GreaterEquals(b, y))
            | (Expr::Smaller(a, x), Expr::Smaller(b, y))
            | (Expr::SmallerEquals(a, x), Expr::SmallerEquals(b, y)) => a == b && x == y,
            (Expr::Regex(a, x), Expr::Regex(b, y)) | (Expr::NotRegex(a, x), Expr::NotRegex(b, y)) => a == b && x.source == y.source,
            (Expr::And(a, _), Expr::And(b, _)) | (Expr::Or(a, _), Expr::Or(b, _)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y)),
            _ => false,
        }
    }

    pub fn substitute_constants(&self) -> Option<Expr> {
        match self {
            Expr::Defined(key) => Some(constant(key).map_or_else(|| self.clone(), Expr::boolean)),
            Expr::Not(key) => Some(constant(key).map_or_else(|| self.clone(), |value| Expr::boolean(!value))),
            Expr::Equals(key, value) => Some(constant(key).map_or_else(|| self.clone(), |c| Expr::boolean(*value == Value::String(c.to_string())))),
            Expr::NotEquals(key, value) => Some(constant(key).map_or_else(|| self.clone(), |c| Expr::boolean(*value != Value::String(c.to_string())))),
            Expr::And(terms, negated) => and_create(terms.iter().map(Expr::substitute_constants).collect(), negated.clone(), false),
            Expr::Or(terms, negated) => or_create(terms.iter().map(Expr::substitute_constants).collect(), negated.clone(), false),
            _ => Some(self.clone()),
        }
    }

    pub fn evaluate(&self, context: &dyn Context) -> bool {
        match self {
            Expr::False => false,
            Expr::True => true,
            Expr::Defined(key) => truthy(context.value(key)),
            Expr::Not(key) => !truthy(context.value(key)),
            Expr::Equals(key, value) => loose_equals(context.value(key), value),
            Expr::NotEquals(key, value) => !loose_equals(context.value(key), value),
            Expr::And(terms, _) => terms.iter().all(|term| term.evaluate(context)),
            Expr::Or(terms, _) => terms.iter().any(|term| term.evaluate(context)),
            Expr::Regex(key, regex) => regex.test(context.value(key)),
            Expr::NotRegex(key, regex) => !regex.test(context.value(key)),
            Expr::In(key, value_key) => evaluate_in(context, key, value_key),
            Expr::NotIn(key, value_key) => !evaluate_in(context, key, value_key),
            Expr::Greater(key, Scalar::Number(value)) => parse_float(&js_string(context.value(key))) > *value,
            Expr::GreaterEquals(key, Scalar::Number(value)) => parse_float(&js_string(context.value(key))) >= *value,
            Expr::Smaller(key, Scalar::Number(value)) => parse_float(&js_string(context.value(key))) < *value,
            Expr::SmallerEquals(key, Scalar::Number(value)) => parse_float(&js_string(context.value(key))) <= *value,
            Expr::Greater(..) | Expr::GreaterEquals(..) | Expr::Smaller(..) | Expr::SmallerEquals(..) => false,
        }
    }

    #[cfg(test)]
    pub fn serialize(&self) -> String {
        match self {
            Expr::False => "false".into(),
            Expr::True => "true".into(),
            Expr::Defined(key) => key.clone(),
            Expr::Not(key) => format!("!{key}"),
            Expr::Equals(key, value) => format!("{key} == '{}'", js_string(Some(value))),
            Expr::NotEquals(key, value) => format!("{key} != '{}'", js_string(Some(value))),
            Expr::And(terms, _) => terms.iter().map(Expr::serialize).collect::<Vec<_>>().join(" && "),
            Expr::Or(terms, _) => terms.iter().map(Expr::serialize).collect::<Vec<_>>().join(" || "),
            Expr::Regex(key, regex) => format!("{key} =~ /{}/{}", regex.source, regex.flags),
            Expr::NotRegex(key, regex) => format!("!({key} =~ /{}/{})", regex.source, regex.flags),
            Expr::In(key, value_key) => format!("{key} in '{value_key}'"),
            Expr::NotIn(key, value_key) => format!("{key} not in '{value_key}'"),
            Expr::Greater(key, value) => format!("{key} > {}", scalar_string(value)),
            Expr::GreaterEquals(key, value) => format!("{key} >= {}", scalar_string(value)),
            Expr::Smaller(key, value) => format!("{key} < {}", scalar_string(value)),
            Expr::SmallerEquals(key, value) => format!("{key} <= {}", scalar_string(value)),
        }
    }

    pub fn negate(&self) -> Expr {
        match self {
            Expr::False => Expr::True,
            Expr::True => Expr::False,
            Expr::Defined(key) => Expr::not(key),
            Expr::Not(key) => Expr::has(key),
            Expr::Equals(key, value) => Expr::key_not_equals(key, value.clone()),
            Expr::NotEquals(key, value) => Expr::key_equals(key, value.clone()),
            Expr::Regex(key, regex) => Expr::NotRegex(key.clone(), regex.clone()),
            Expr::NotRegex(key, regex) => Expr::Regex(key.clone(), regex.clone()),
            Expr::In(key, value_key) => Expr::NotIn(key.clone(), value_key.clone()),
            Expr::NotIn(key, value_key) => Expr::In(key.clone(), value_key.clone()),
            Expr::Greater(key, value) => Expr::SmallerEquals(key.clone(), value.clone()),
            Expr::GreaterEquals(key, value) => Expr::Smaller(key.clone(), value.clone()),
            Expr::Smaller(key, value) => Expr::GreaterEquals(key.clone(), value.clone()),
            Expr::SmallerEquals(key, value) => Expr::Greater(key.clone(), value.clone()),
            Expr::And(_, Some(negated)) | Expr::Or(_, Some(negated)) => (**negated).clone(),
            Expr::And(terms, None) => or_create(terms.iter().map(|term| Some(term.negate())).collect(), Some(Arc::new(self.clone())), true).expect("an AND has terms"),
            Expr::Or(terms, None) => {
                // No parentheses: distribute the AND of the negated terms over their OR terminals, two at a time.
                let mut result: Vec<Expr> = terms.iter().map(Expr::negate).collect();
                while result.len() > 1 {
                    let left = result.remove(0);
                    let right = result.remove(0);
                    let mut all = Vec::new();
                    for l in terminals(&left) {
                        for r in terminals(&right) {
                            all.push(and_create(vec![Some(l.clone()), Some(r.clone())], None, false));
                        }
                    }
                    result.insert(0, or_create(all, None, false).expect("terms were given"));
                }
                or_create(result.into_iter().map(Some).collect(), Some(Arc::new(self.clone())), true).expect("terms were given")
            }
        }
    }
}

fn terminals(expr: &Expr) -> Vec<Expr> {
    match expr {
        Expr::Or(terms, _) => terms.clone(),
        _ => vec![expr.clone()],
    }
}

fn with_float_or_str(value: Value, create: impl FnOnce(Scalar) -> Expr) -> Expr {
    match value {
        Value::String(text) => {
            let number = parse_float(&text);
            create(if number.is_nan() { Scalar::String(text) } else { Scalar::Number(number) })
        }
        Value::Number(number) => create(Scalar::Number(number.as_f64().expect("JSON numbers are finite"))),
        _ => Expr::False,
    }
}

fn sort_dedup(terms: &mut Vec<Expr>) {
    terms.sort_by(Expr::cmp);
    terms.dedup_by(|b, a| a.equals(b));
}

/// `ContextKeyAndExpr._normalizeArr`.
fn and_create(terms: Vec<Option<Expr>>, negated: Negation, extra_redundant_check: bool) -> Option<Expr> {
    let mut expr = Vec::new();
    let mut has_true = false;
    for term in terms.into_iter().flatten() {
        match term {
            Expr::True => has_true = true,
            Expr::False => return Some(Expr::False),
            Expr::And(inner, _) => expr.extend(inner),
            term => expr.push(term),
        }
    }
    if expr.is_empty() {
        return has_true.then_some(Expr::True);
    }
    if expr.len() == 1 {
        return expr.pop();
    }
    sort_dedup(&mut expr);
    if expr.len() == 1 {
        return expr.pop();
    }
    // No parentheses: distribute the last OR over the term before it until no OR is left.
    while expr.len() > 1 {
        let Some(or_index) = expr.iter().rposition(|term| matches!(term, Expr::Or(..))) else { break };
        let Expr::Or(or_terms, _) = expr.remove(or_index) else { unreachable!() };
        let second_to_last = expr.pop().expect("at least one term is left");
        let is_finished = expr.is_empty();
        let distributed = or_terms.into_iter().map(|term| and_create(vec![Some(term), Some(second_to_last.clone())], None, extra_redundant_check)).collect();
        if let Some(result) = or_create(distributed, None, is_finished) {
            if result.is_false() {
                return Some(Expr::False);
            }
            expr.push(result);
            expr.sort_by(Expr::cmp);
        }
    }
    if expr.len() == 1 {
        return expr.pop();
    }
    if expr.iter().any(|term| matches!(term, Expr::And(..))) {
        // A distributed OR that collapsed into a single AND must not stay nested.
        let mut flat: Vec<Expr> = expr.drain(..).flat_map(|term| if let Expr::And(inner, _) = term { inner } else { vec![term] }).collect();
        flat.sort_by(Expr::cmp);
        for term in flat {
            if expr.last().is_none_or(|last: &Expr| !last.equals(&term)) {
                expr.push(term);
            }
        }
    }
    if extra_redundant_check {
        for i in 0..expr.len() {
            for j in i + 1..expr.len() {
                if expr[i].negate().equals(&expr[j]) {
                    return Some(Expr::False);
                }
            }
        }
        if expr.len() == 1 {
            return expr.pop();
        }
    }
    Some(Expr::And(expr, negated))
}

/// `ContextKeyOrExpr._normalizeArr`.
fn or_create(terms: Vec<Option<Expr>>, negated: Negation, extra_redundant_check: bool) -> Option<Expr> {
    let mut expr = Vec::new();
    let mut has_false = false;
    for term in terms.into_iter().flatten() {
        match term {
            Expr::False => has_false = true,
            Expr::True => return Some(Expr::True),
            Expr::Or(inner, _) => expr.extend(inner),
            term => expr.push(term),
        }
    }
    if expr.is_empty() {
        return has_false.then_some(Expr::False);
    }
    expr.sort_by(Expr::cmp);
    if expr.len() == 1 {
        return expr.pop();
    }
    sort_dedup(&mut expr);
    if expr.len() == 1 {
        return expr.pop();
    }
    if extra_redundant_check {
        for i in 0..expr.len() {
            for j in i + 1..expr.len() {
                if expr[i].negate().equals(&expr[j]) {
                    return Some(Expr::True);
                }
            }
        }
        if expr.len() == 1 {
            return expr.pop();
        }
    }
    Some(Expr::Or(expr, negated))
}

/// True if it is provable that `p` implies `q`.
pub fn implies(p: &Expr, q: &Expr) -> bool {
    if p.is_false() || q.is_true() {
        return true;
    }
    if let Expr::Or(p_terms, _) = p {
        return matches!(q, Expr::Or(q_terms, _) if all_elements_included(p_terms, q_terms));
    }
    if let Expr::Or(q_terms, _) = q {
        return q_terms.iter().any(|term| implies(p, term));
    }
    if let Expr::And(p_terms, _) = p {
        if let Expr::And(q_terms, _) = q {
            return all_elements_included(q_terms, p_terms);
        }
        return p_terms.iter().any(|term| implies(term, q));
    }
    p.equals(q)
}

/// Every element of sorted `p` is in sorted `q`.
fn all_elements_included(p: &[Expr], q: &[Expr]) -> bool {
    let (mut p_index, mut q_index) = (0, 0);
    while p_index < p.len() && q_index < q.len() {
        match p[p_index].cmp(&q[q_index]) {
            Ordering::Less => return false,
            Ordering::Equal => {
                p_index += 1;
                q_index += 1;
            }
            Ordering::Greater => q_index += 1,
        }
    }
    p_index == p.len()
}

fn evaluate_in(context: &dyn Context, key: &str, value_key: &str) -> bool {
    let item = context.value(key);
    match context.value(value_key) {
        Some(Value::Array(source)) => item.is_some_and(|item| source.iter().any(|element| same_value_zero(element, item))),
        Some(Value::Object(source)) => matches!(item, Some(Value::String(item)) if source.contains_key(item)),
        _ => false,
    }
}

fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        // Arrays and objects compare by identity in JavaScript; values from two places are never the same object.
        (Value::Array(_) | Value::Object(_), _) => false,
        _ => a == b,
    }
}

// JavaScript value semantics the expressions rely on.

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0. && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `String(value)`.
pub fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(number)) => number_string(number.as_f64().expect("JSON numbers are finite")),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items.iter().map(|item| if item.is_null() { String::new() } else { js_string(Some(item)) }).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// `Number.prototype.toString()` for the values context keys hold.
fn number_string(number: f64) -> String {
    if number.is_nan() {
        return "NaN".into();
    }
    if number.is_infinite() {
        return if number > 0. { "Infinity".into() } else { "-Infinity".into() };
    }
    if number == number.trunc() && number.abs() < 1e21 {
        return format!("{number:.0}");
    }
    format!("{number}")
}

#[cfg(test)]
fn scalar_string(value: &Scalar) -> String {
    match value {
        Scalar::Number(number) => number_string(*number),
        Scalar::String(text) => text.clone(),
    }
}

/// `ToNumber`.
fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.,
        Some(Value::Bool(value)) => f64::from(u8::from(*value)),
        Some(Value::Number(number)) => number.as_f64().expect("JSON numbers are finite"),
        Some(Value::String(text)) => string_to_number(text),
        Some(value @ (Value::Array(_) | Value::Object(_))) => string_to_number(&js_string(Some(value))),
    }
}

fn string_to_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.;
    }
    match text {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if text.starts_with("0x") || text.starts_with("0X") => i64::from_str_radix(&text[2..], 16).map_or(f64::NAN, |n| n as f64),
        _ if text.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) => text.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// `parseFloat`: the longest leading decimal literal.
pub fn parse_float(text: &str) -> f64 {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    if text[end..].starts_with("Infinity") {
        return if text.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    if end == digits_start || &text[digits_start..end] == "." {
        return f64::NAN;
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut exponent = end + 1;
        if exponent < bytes.len() && (bytes[exponent] == b'+' || bytes[exponent] == b'-') {
            exponent += 1;
        }
        let exponent_digits = exponent;
        while exponent < bytes.len() && bytes[exponent].is_ascii_digit() {
            exponent += 1;
        }
        if exponent > exponent_digits {
            end = exponent;
        }
    }
    text[..end].parse().unwrap_or(f64::NAN)
}

/// Abstract equality `a == b` with `a` from the context.
fn loose_equals(a: Option<&Value>, b: &Value) -> bool {
    match (a, b) {
        (None | Some(Value::Null), Value::Null) => true,
        (None | Some(Value::Null), _) | (_, Value::Null) => false,
        (Some(Value::String(x)), Value::String(y)) => x == y,
        (Some(Value::Bool(x)), Value::Bool(y)) => x == y,
        (Some(Value::Array(_) | Value::Object(_)), Value::Array(_) | Value::Object(_)) => false,
        (Some(x @ (Value::Array(_) | Value::Object(_))), Value::String(y)) => js_string(Some(x)) == *y,
        (Some(Value::String(x)), y @ (Value::Array(_) | Value::Object(_))) => *x == js_string(Some(y)),
        (Some(x), y) => to_number(Some(x)) == to_number(Some(y)),
    }
}

enum Prim {
    Number(f64),
    String(String),
}

fn prim(value: &Value) -> Prim {
    match value {
        Value::String(_) | Value::Array(_) | Value::Object(_) => Prim::String(js_string(Some(value))),
        _ => Prim::Number(to_number(Some(value))),
    }
}

fn scalar_prim(value: &Scalar) -> Prim {
    match value {
        Scalar::Number(number) => Prim::Number(*number),
        Scalar::String(text) => Prim::String(text.clone()),
    }
}

/// `cmp2`'s `<` and `>`: equal when neither holds (NaN included).
fn js_compare(a: &Prim, b: &Prim) -> Ordering {
    match (a, b) {
        (Prim::String(x), Prim::String(y)) => utf16_cmp(x, y),
        _ => {
            let number = |p: &Prim| match p {
                Prim::Number(n) => *n,
                Prim::String(s) => string_to_number(s),
            };
            number(a).partial_cmp(&number(b)).unwrap_or(Ordering::Equal)
        }
    }
}

/// JavaScript compares strings by UTF-16 code units.
fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

// Scanner (scanner.ts).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenType {
    LParen,
    RParen,
    Neg,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    RegexOp,
    RegexStr,
    True,
    False,
    In,
    Not,
    And,
    Or,
    Str,
    QuotedStr,
    Error,
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenType,
    /// UTF-16 offset, as upstream reports it.
    offset: usize,
    lexeme: String,
    is_triple_eq: bool,
}

impl Token {
    fn lexeme(&self) -> String {
        match self.kind {
            TokenType::LParen => "(".into(),
            TokenType::RParen => ")".into(),
            TokenType::Neg => "!".into(),
            TokenType::Eq => if self.is_triple_eq { "===" } else { "==" }.into(),
            TokenType::NotEq => if self.is_triple_eq { "!==" } else { "!=" }.into(),
            TokenType::Lt => "<".into(),
            TokenType::LtEq => "<=".into(),
            TokenType::Gt => ">".into(),
            TokenType::GtEq => ">=".into(),
            TokenType::RegexOp => "=~".into(),
            TokenType::True => "true".into(),
            TokenType::False => "false".into(),
            TokenType::In => "in".into(),
            TokenType::Not => "not".into(),
            TokenType::And => "&&".into(),
            TokenType::Or => "||".into(),
            TokenType::Eof => "EOF".into(),
            TokenType::RegexStr | TokenType::Str | TokenType::QuotedStr | TokenType::Error => self.lexeme.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LexingError {
    pub offset: usize,
    pub lexeme: String,
    pub additional_info: Option<String>,
}

fn hint_did_you_mean(meant: &[&str]) -> Option<String> {
    match meant {
        [a] => Some(format!("Did you mean {a}?")),
        [a, b] => Some(format!("Did you mean {a} or {b}?")),
        [a, b, c] => Some(format!("Did you mean {a}, {b} or {c}?")),
        _ => None,
    }
}

const HINT_QUOTE: &str = "Did you forget to open or close the quote?";
const HINT_ESCAPE_SLASH: &str = "Did you forget to escape the '/' (slash) character? Put two backslashes before it to escape, e.g., '\\\\/'.";

struct Scanner {
    input: Vec<char>,
    /// UTF-16 offset of each char, plus the end.
    offsets: Vec<usize>,
    start: usize,
    current: usize,
    tokens: Vec<Token>,
    errors: Vec<LexingError>,
}

/// `[a-zA-Z0-9_<>\-\./\\:\*\?\+\[\]\^,#@;"%\$\p{L}-]`
fn is_string_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "_<>-./\\:*?+[]^,#@;\"%$".contains(ch) || (!ch.is_ascii() && ch.is_alphabetic())
}

impl Scanner {
    fn scan(input: &str) -> (Vec<Token>, Vec<LexingError>) {
        let input: Vec<char> = input.chars().collect();
        let mut offsets = Vec::with_capacity(input.len() + 1);
        let mut offset = 0;
        for ch in &input {
            offsets.push(offset);
            offset += ch.len_utf16();
        }
        offsets.push(offset);
        let mut scanner = Scanner { input, offsets, start: 0, current: 0, tokens: Vec::new(), errors: Vec::new() };
        while !scanner.is_at_end() {
            scanner.start = scanner.current;
            let ch = scanner.advance();
            match ch {
                '(' => scanner.add_token(TokenType::LParen),
                ')' => scanner.add_token(TokenType::RParen),
                '!' => {
                    if scanner.matches('=') {
                        let is_triple_eq = scanner.matches('=');
                        scanner.push(TokenType::NotEq, String::new(), is_triple_eq);
                    } else {
                        scanner.add_token(TokenType::Neg);
                    }
                }
                '\'' => scanner.quoted_string(),
                '/' => scanner.regex(),
                '=' => {
                    if scanner.matches('=') {
                        let is_triple_eq = scanner.matches('=');
                        scanner.push(TokenType::Eq, String::new(), is_triple_eq);
                    } else if scanner.matches('~') {
                        scanner.add_token(TokenType::RegexOp);
                    } else {
                        scanner.error(hint_did_you_mean(&["==", "=~"]));
                    }
                }
                '<' => {
                    let kind = if scanner.matches('=') { TokenType::LtEq } else { TokenType::Lt };
                    scanner.add_token(kind);
                }
                '>' => {
                    let kind = if scanner.matches('=') { TokenType::GtEq } else { TokenType::Gt };
                    scanner.add_token(kind);
                }
                '&' => {
                    if scanner.matches('&') {
                        scanner.add_token(TokenType::And);
                    } else {
                        scanner.error(hint_did_you_mean(&["&&"]));
                    }
                }
                '|' => {
                    if scanner.matches('|') {
                        scanner.add_token(TokenType::Or);
                    } else {
                        scanner.error(hint_did_you_mean(&["||"]));
                    }
                }
                ' ' | '\r' | '\t' | '\n' | '\u{a0}' => {}
                _ => scanner.string(),
            }
        }
        scanner.start = scanner.current;
        scanner.add_token(TokenType::Eof);
        (scanner.tokens, scanner.errors)
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.input.len()
    }

    fn advance(&mut self) -> char {
        self.current += 1;
        self.input[self.current - 1]
    }

    fn matches(&mut self, expected: char) -> bool {
        if self.is_at_end() || self.input[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn text(&self, from: usize, to: usize) -> String {
        self.input[from..to].iter().collect()
    }

    fn push(&mut self, kind: TokenType, lexeme: String, is_triple_eq: bool) {
        self.tokens.push(Token { kind, offset: self.offsets[self.start], lexeme, is_triple_eq });
    }

    fn add_token(&mut self, kind: TokenType) {
        self.push(kind, String::new(), false);
    }

    fn error(&mut self, additional_info: Option<String>) {
        let lexeme = self.text(self.start, self.current);
        self.errors.push(LexingError { offset: self.offsets[self.start], lexeme: lexeme.clone(), additional_info });
        self.push(TokenType::Error, lexeme, false);
    }

    /// A char outside the string class is skipped without a token, as the sticky regex fails upstream.
    fn string(&mut self) {
        if !is_string_char(self.input[self.start]) {
            return;
        }
        let mut end = self.start;
        while end < self.input.len() && is_string_char(self.input[end]) {
            end += 1;
        }
        self.current = end;
        let lexeme = self.text(self.start, end);
        let keyword = match lexeme.as_str() {
            "not" => Some(TokenType::Not),
            "in" => Some(TokenType::In),
            "false" => Some(TokenType::False),
            "true" => Some(TokenType::True),
            _ => None,
        };
        match keyword {
            Some(kind) => self.add_token(kind),
            None => self.push(TokenType::Str, lexeme, false),
        }
    }

    fn quoted_string(&mut self) {
        while !self.is_at_end() && self.input[self.current] != '\'' {
            self.current += 1;
        }
        if self.is_at_end() {
            self.error(Some(HINT_QUOTE.into()));
            return;
        }
        self.current += 1;
        let lexeme = self.text(self.start + 1, self.current - 1);
        self.tokens.push(Token { kind: TokenType::QuotedStr, offset: self.offsets[self.start + 1], lexeme, is_triple_eq: false });
    }

    fn regex(&mut self) {
        let mut p = self.current;
        let mut in_escape = false;
        let mut in_class = false;
        loop {
            if p >= self.input.len() {
                self.current = p;
                self.error(Some(HINT_ESCAPE_SLASH.into()));
                return;
            }
            let ch = self.input[p];
            if in_escape {
                in_escape = false;
            } else if ch == '/' && !in_class {
                p += 1;
                break;
            } else if ch == '[' {
                in_class = true;
            } else if ch == '\\' {
                in_escape = true;
            } else if ch == ']' {
                in_class = false;
            }
            p += 1;
        }
        while p < self.input.len() && "igsmyu".contains(self.input[p]) {
            p += 1;
        }
        self.current = p;
        let lexeme = self.text(self.start, p);
        self.push(TokenType::RegexStr, lexeme, false);
    }
}

// Parser (contextkey.ts `Parser`).

#[derive(Clone, Debug, PartialEq)]
pub struct ParsingError {
    pub message: String,
    pub offset: usize,
    pub lexeme: String,
    pub additional_info: Option<String>,
}

pub struct Parser {
    regex_parsing_with_error_recovery: bool,
    tokens: Vec<Token>,
    current: usize,
    pub lexing_errors: Vec<LexingError>,
    pub parsing_errors: Vec<ParsingError>,
}

/// `Parser._parseError`: the errors are in `parsing_errors`.
struct ParseError;

type ParseResult = Result<Option<Expr>, ParseError>;

impl Parser {
    pub fn new(regex_parsing_with_error_recovery: bool) -> Parser {
        Parser { regex_parsing_with_error_recovery, tokens: Vec::new(), current: 0, lexing_errors: Vec::new(), parsing_errors: Vec::new() }
    }

    /// `None` on an error; `lexing_errors` and `parsing_errors` say which.
    pub fn parse(&mut self, input: &str) -> Option<Expr> {
        if input.is_empty() {
            self.parsing_errors.push(ParsingError {
                message: "Empty context key expression".into(),
                offset: 0,
                lexeme: String::new(),
                additional_info: Some("Did you forget to write an expression? You can also put 'false' or 'true' to always evaluate to false or true, respectively.".into()),
            });
            return None;
        }
        let (tokens, lexing_errors) = Scanner::scan(input);
        self.tokens = tokens;
        self.lexing_errors = lexing_errors;
        self.current = 0;
        self.parsing_errors = Vec::new();
        let expr = self.or().ok()?;
        if !self.is_at_end() {
            let peek = self.peek().clone();
            let additional_info = (peek.kind == TokenType::Str).then(|| "Did you forget to put && or || before the token?".to_string());
            self.parsing_errors.push(ParsingError { message: "Unexpected token".into(), offset: peek.offset, lexeme: peek.lexeme(), additional_info });
            return None;
        }
        expr
    }

    fn or(&mut self) -> ParseResult {
        let mut expr = vec![self.and()?];
        while self.match_one(TokenType::Or) {
            expr.push(self.and()?);
        }
        Ok(if expr.len() == 1 { expr.pop().unwrap() } else { Expr::or(expr) })
    }

    fn and(&mut self) -> ParseResult {
        let mut expr = vec![self.term()?];
        while self.match_one(TokenType::And) {
            expr.push(self.term()?);
        }
        Ok(if expr.len() == 1 { expr.pop().unwrap() } else { Expr::and(expr) })
    }

    fn term(&mut self) -> ParseResult {
        if !self.match_one(TokenType::Neg) {
            return self.primary();
        }
        let peek = self.peek().clone();
        match peek.kind {
            TokenType::True => {
                self.advance();
                Ok(Some(Expr::False))
            }
            TokenType::False => {
                self.advance();
                Ok(Some(Expr::True))
            }
            TokenType::LParen => {
                self.advance();
                let expr = self.or()?;
                self.consume(TokenType::RParen, "closing parenthesis ')'")?;
                Ok(expr.map(|expr| expr.negate()))
            }
            TokenType::Str => {
                self.advance();
                Ok(Some(Expr::not(&peek.lexeme)))
            }
            _ => Err(self.expected_but_got("KEY | true | false | '(' expression ')'", &peek)),
        }
    }

    fn primary(&mut self) -> ParseResult {
        let peek = self.peek().clone();
        match peek.kind {
            TokenType::True => {
                self.advance();
                Ok(Some(Expr::True))
            }
            TokenType::False => {
                self.advance();
                Ok(Some(Expr::False))
            }
            TokenType::LParen => {
                self.advance();
                let expr = self.or()?;
                self.consume(TokenType::RParen, "closing parenthesis ')'")?;
                Ok(expr)
            }
            TokenType::Str => {
                let key = peek.lexeme;
                self.advance();
                if self.match_one(TokenType::RegexOp) {
                    return self.regex_value(&key).map(Some);
                }
                if self.match_one(TokenType::Not) {
                    self.consume(TokenType::In, "'in' after 'not'.")?;
                    let right = self.value();
                    return Ok(Some(Expr::not_in(&key, &right)));
                }
                let op = self.peek().kind;
                let comparison: Option<fn(&str, Value) -> Expr> = match op {
                    TokenType::Lt => Some(Expr::smaller),
                    TokenType::LtEq => Some(Expr::smaller_equals),
                    TokenType::Gt => Some(Expr::greater),
                    TokenType::GtEq => Some(Expr::greater_equals),
                    _ => None,
                };
                if let Some(create) = comparison {
                    self.advance();
                    return Ok(Some(create(&key, Value::String(self.value()))));
                }
                match op {
                    TokenType::Eq | TokenType::NotEq => {
                        self.advance();
                        let right = self.value();
                        let positive = op == TokenType::Eq;
                        // "foo == 'true'" stays an equality; "foo == true" becomes "foo", as the old parser did.
                        let quoted = self.previous().kind == TokenType::QuotedStr;
                        Ok(Some(match (quoted, right.as_str()) {
                            (false, "true") => if positive { Expr::has(&key) } else { Expr::not(&key) },
                            (false, "false") => if positive { Expr::not(&key) } else { Expr::has(&key) },
                            _ if positive => Expr::key_equals(&key, Value::String(right)),
                            _ => Expr::key_not_equals(&key, Value::String(right)),
                        }))
                    }
                    TokenType::In => {
                        self.advance();
                        let right = self.value();
                        Ok(Some(Expr::is_in(&key, &right)))
                    }
                    _ => Ok(Some(Expr::has(&key))),
                }
            }
            TokenType::Eof => {
                self.parsing_errors.push(ParsingError {
                    message: "Unexpected end of expression".into(),
                    offset: peek.offset,
                    lexeme: String::new(),
                    additional_info: Some("Did you forget to put a context key?".into()),
                });
                Err(ParseError)
            }
            _ => Err(self.expected_but_got(
                "true | false | KEY \n\t| KEY '=~' REGEX \n\t| KEY ('==' | '!=' | '<' | '<=' | '>' | '>=' | 'in' | 'not' 'in') value",
                &peek,
            )),
        }
    }

    fn regex_value(&mut self, key: &str) -> Result<Expr, ParseError> {
        let expr = self.peek().clone();
        if !self.regex_parsing_with_error_recovery {
            self.advance();
            if expr.kind != TokenType::RegexStr {
                return Err(self.expected_but_got("REGEX", &expr));
            }
            return match regex_from_lexeme(&expr.lexeme) {
                Some(regex) => Ok(Expr::regex(key, regex)),
                None => Err(self.expected_but_got("REGEX", &expr)),
            };
        }
        match expr.kind {
            // An Error token too, for input such as /(/file)/.
            TokenType::RegexStr | TokenType::Error => {
                let mut reconstruction = vec![expr.lexeme.clone()];
                self.advance();
                let mut following = self.peek().clone();
                let mut paren_balance: i32 = 0;
                for ch in expr.lexeme.chars() {
                    match ch {
                        '(' => paren_balance += 1,
                        ')' => paren_balance -= 1,
                        _ => {}
                    }
                }
                while !self.is_at_end() && following.kind != TokenType::And && following.kind != TokenType::Or {
                    match following.kind {
                        TokenType::LParen => paren_balance += 1,
                        TokenType::RParen => paren_balance -= 1,
                        TokenType::RegexStr | TokenType::QuotedStr => {
                            // Upstream counts ')' at the same index of the first lexeme (contextkey.ts), kept as is.
                            let first: Vec<char> = expr.lexeme.chars().collect();
                            for (i, ch) in following.lexeme.chars().enumerate() {
                                if ch == '(' {
                                    paren_balance += 1;
                                } else if first.get(i) == Some(&')') {
                                    paren_balance -= 1;
                                }
                            }
                        }
                        _ => {}
                    }
                    if paren_balance < 0 {
                        break;
                    }
                    reconstruction.push(following.lexeme());
                    self.advance();
                    following = self.peek().clone();
                }
                match regex_from_lexeme(&reconstruction.concat()) {
                    Some(regex) => Ok(Expr::regex(key, regex)),
                    None => Err(self.expected_but_got("REGEX", &expr)),
                }
            }
            TokenType::QuotedStr => {
                let serialized = expr.lexeme.clone();
                self.advance();
                let mut regex = None;
                if !serialized.trim().is_empty() {
                    let start = serialized.find('/');
                    let end = serialized.rfind('/');
                    if let (Some(start), Some(end)) = (start, end)
                        && start != end
                    {
                        let value = &serialized[start + 1..end];
                        let ignore_case = if serialized[end + 1..].starts_with('i') { "i" } else { "" };
                        match JsRegex::new(value, ignore_case) {
                            Some(compiled) => regex = Some(compiled),
                            None => return Err(self.expected_but_got("REGEX", &expr)),
                        }
                    }
                }
                match regex {
                    Some(regex) => Ok(Expr::regex(key, regex)),
                    None => Err(self.expected_but_got("REGEX", &expr)),
                }
            }
            _ => {
                let peek = self.peek().clone();
                Err(self.expected_but_got("REGEX", &peek))
            }
        }
    }

    /// A missing value is `''` and eats nothing: extensions ship `"when": "foo == "`.
    fn value(&mut self) -> String {
        let token = self.peek().clone();
        match token.kind {
            TokenType::Str | TokenType::QuotedStr => {
                self.advance();
                token.lexeme
            }
            TokenType::True | TokenType::False | TokenType::In => {
                self.advance();
                token.lexeme()
            }
            _ => String::new(),
        }
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current - 1]
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn is_at_end(&self) -> bool {
        self.peek().kind == TokenType::Eof
    }

    fn advance(&mut self) {
        if !self.is_at_end() {
            self.current += 1;
        }
    }

    fn match_one(&mut self, kind: TokenType) -> bool {
        if self.peek().kind == kind {
            self.advance();
            return true;
        }
        false
    }

    fn consume(&mut self, kind: TokenType, expected: &str) -> Result<(), ParseError> {
        if self.peek().kind == kind {
            self.advance();
            return Ok(());
        }
        let peek = self.peek().clone();
        Err(self.expected_but_got(expected, &peek))
    }

    fn expected_but_got(&mut self, expected: &str, got: &Token) -> ParseError {
        let lexeme = got.lexeme();
        self.parsing_errors.push(ParsingError { message: format!("Expected: {expected}\nReceived: '{lexeme}'."), offset: got.offset, lexeme, additional_info: None });
        ParseError
    }
}

/// `/REGEX/FLAGS` with the `g` and `y` flags dropped, as the parser does.
fn regex_from_lexeme(lexeme: &str) -> Option<JsRegex> {
    let closing = lexeme.rfind('/')?;
    let flags: String = lexeme[closing + 1..].chars().filter(|flag| *flag != 'g' && *flag != 'y').collect();
    // JavaScript's substring(1, 0) swaps its bounds and yields the leading '/'.
    let pattern = if closing >= 1 { &lexeme[1..closing] } else { &lexeme[..1] };
    JsRegex::new(pattern, &flags)
}

#[cfg(test)]
mod tests;
