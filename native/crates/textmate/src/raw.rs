//! Raw grammars as `parseRawGrammar` reads them (JSON, or the plist XML of the JavaScript regex
//! grammar). Rules are shared `Rc`s: a rule compiles once, then every include of it reuses its id,
//! as vscode-textmate caches the id on the raw rule object.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{Map, Value};

pub type Repository = Rc<HashMap<String, Rc<RawRule>>>;

#[derive(Default)]
pub struct RawRule {
    pub id: Cell<Option<usize>>,
    pub include: Option<String>,
    pub name: Option<String>,
    pub content_name: Option<String>,
    pub match_: Option<String>,
    pub captures: Option<RawCaptures>,
    pub begin: Option<String>,
    pub begin_captures: Option<RawCaptures>,
    pub end: Option<String>,
    pub end_captures: Option<RawCaptures>,
    pub while_: Option<String>,
    pub while_captures: Option<RawCaptures>,
    pub patterns: Option<Vec<Rc<RawRule>>>,
    pub repository: Option<Repository>,
    pub apply_end_pattern_last: bool,
}

/// Capture number to rule. Keys are read as `parseInt` reads them; one that is not a number is
/// dropped, as `r[NaN]` never becomes an array slot.
pub type RawCaptures = Vec<(usize, Rc<RawRule>)>;

pub struct RawGrammar {
    pub scope_name: String,
    pub repository: Repository,
    pub injections: Vec<(String, Rc<RawRule>)>,
    pub injection_selector: Option<String>,
    /// The grammar object itself as a rule, which an injection grammar is compiled from; it shares
    /// its patterns with `$self`.
    pub as_rule: Rc<RawRule>,
}

pub struct RawGrammarSource {
    pub scope_name: String,
    patterns: Vec<Rc<RawRule>>,
    repository: HashMap<String, Rc<RawRule>>,
    injections: Vec<(String, Rc<RawRule>)>,
    injection_selector: Option<String>,
}

impl RawGrammarSource {
    pub fn parse(content: &str, path: &str) -> Result<RawGrammarSource, String> {
        let value = if path.ends_with(".json") {
            serde_json::from_str(content).map_err(|e| e.to_string())?
        } else {
            crate::plist::parse(content)?
        };
        let object = value.as_object().ok_or("grammar is not an object")?;
        Ok(RawGrammarSource {
            scope_name: string(object, "scopeName").unwrap_or_default(),
            patterns: rules(object.get("patterns")).unwrap_or_default(),
            repository: repository(object.get("repository")).unwrap_or_default(),
            injections: object.get("injections").and_then(Value::as_object).map(|map| map.iter().filter_map(|(k, v)| Some((k.clone(), rule(v)?))).collect()).unwrap_or_default(),
            injection_selector: string(object, "injectionSelector"),
        })
    }

    /// `initGrammar`: a fresh copy whose `$self` is the top-level patterns and `$base` the includer's base.
    /// The copy gets its own rule objects, so its ids are its own, as `clone(grammar)` does.
    pub fn instantiate(&self, base: Option<Rc<RawRule>>) -> RawGrammar {
        let mut repository: HashMap<String, Rc<RawRule>> = self.repository.iter().map(|(k, v)| (k.clone(), deep_clone(v))).collect();
        let patterns: Vec<Rc<RawRule>> = self.patterns.iter().map(deep_clone).collect();
        let as_rule = Rc::new(RawRule { patterns: Some(patterns.clone()), ..Default::default() });
        let self_rule = Rc::new(RawRule { name: Some(self.scope_name.clone()), patterns: Some(patterns), ..Default::default() });
        repository.insert("$base".into(), base.unwrap_or_else(|| self_rule.clone()));
        repository.insert("$self".into(), self_rule);
        RawGrammar {
            scope_name: self.scope_name.clone(),
            repository: Rc::new(repository),
            injections: self.injections.iter().map(|(k, v)| (k.clone(), deep_clone(v))).collect(),
            injection_selector: self.injection_selector.clone(),
            as_rule,
        }
    }
}

fn deep_clone(rule: &Rc<RawRule>) -> Rc<RawRule> {
    let captures = |c: &Option<RawCaptures>| c.as_ref().map(|c| c.iter().map(|(i, r)| (*i, deep_clone(r))).collect());
    Rc::new(RawRule {
        id: Cell::new(None),
        include: rule.include.clone(),
        name: rule.name.clone(),
        content_name: rule.content_name.clone(),
        match_: rule.match_.clone(),
        captures: captures(&rule.captures),
        begin: rule.begin.clone(),
        begin_captures: captures(&rule.begin_captures),
        end: rule.end.clone(),
        end_captures: captures(&rule.end_captures),
        while_: rule.while_.clone(),
        while_captures: captures(&rule.while_captures),
        patterns: rule.patterns.as_ref().map(|p| p.iter().map(deep_clone).collect()),
        repository: rule.repository.as_ref().map(|r| Rc::new(r.iter().map(|(k, v)| (k.clone(), deep_clone(v))).collect())),
        apply_end_pattern_last: rule.apply_end_pattern_last,
    })
}

fn string(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

fn rules(value: Option<&Value>) -> Option<Vec<Rc<RawRule>>> {
    Some(value?.as_array()?.iter().filter_map(rule).collect())
}

fn repository(value: Option<&Value>) -> Option<HashMap<String, Rc<RawRule>>> {
    Some(value?.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), rule(v)?))).collect())
}

fn captures(value: Option<&Value>) -> Option<RawCaptures> {
    let entries: Vec<(String, &Value)> = match value? {
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Value::Array(items) => items.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
        _ => return None,
    };
    Some(entries.into_iter().filter_map(|(key, value)| Some((leading_number(&key)?, rule(value)?))).collect())
}

fn leading_number(key: &str) -> Option<usize> {
    let key = key.trim_start();
    let digits = key.bytes().take_while(u8::is_ascii_digit).count();
    key[..digits].parse().ok()
}

/// JavaScript truthiness of `applyEndPatternLast`, which grammars write as `true` or `1`.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
        _ => false,
    }
}

fn rule(value: &Value) -> Option<Rc<RawRule>> {
    let object = value.as_object()?;
    Some(Rc::new(RawRule {
        id: Cell::new(None),
        include: string(object, "include"),
        name: string(object, "name"),
        content_name: string(object, "contentName"),
        match_: string(object, "match"),
        captures: captures(object.get("captures")),
        begin: string(object, "begin"),
        begin_captures: captures(object.get("beginCaptures")),
        end: string(object, "end"),
        end_captures: captures(object.get("endCaptures")),
        while_: string(object, "while"),
        while_captures: captures(object.get("whileCaptures")),
        patterns: rules(object.get("patterns")),
        repository: repository(object.get("repository")).map(Rc::new),
        apply_end_pattern_last: truthy(object.get("applyEndPatternLast")),
    }))
}
