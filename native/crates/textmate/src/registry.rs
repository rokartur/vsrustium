//! Grammar contributions of extensions, as `TMGrammarFactory` and `TMScopeRegistry` keep them, and
//! the grammars built from them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value;

use crate::grammar::{Grammar, GrammarConfiguration, GrammarLookup, StandardTokenType};
use crate::raw::{RawGrammar, RawGrammarSource, RawRule};

struct Definition {
    path: PathBuf,
    embedded_languages: Vec<(String, String)>,
    token_types: Vec<(String, StandardTokenType)>,
    balanced_bracket_selectors: Vec<String>,
    unbalanced_bracket_selectors: Vec<String>,
}

#[derive(Default)]
pub struct Registry {
    /// A later contribution of a scope replaces the earlier one.
    definitions: HashMap<String, Definition>,
    language_to_scope: HashMap<String, String>,
    injections: HashMap<String, Vec<String>>,
    injected_embedded_languages: HashMap<String, Vec<(String, String)>>,
    sources: RefCell<HashMap<String, Option<Rc<RawGrammarSource>>>>,
}

impl Registry {
    /// `extensions`: each extension's folder and its package.json, in registration order.
    pub fn new<'a>(extensions: impl IntoIterator<Item = (&'a Path, &'a Value)>) -> Registry {
        let mut registry = Registry::default();
        for (location, description) in extensions {
            for grammar in description["contributes"]["grammars"].as_array().into_iter().flatten() {
                registry.register(location, grammar);
            }
        }
        registry
    }

    fn register(&mut self, location: &Path, grammar: &Value) {
        let (Some(scope_name), Some(path)) = (grammar["scopeName"].as_str(), grammar["path"].as_str()) else { return };
        let pairs = |key: &str| grammar[key].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect::<Vec<_>>();
        let embedded_languages = pairs("embeddedLanguages");
        let token_types = pairs("tokenTypes")
            .into_iter()
            .filter_map(|(selector, token_type)| {
                let token_type = match token_type.as_str() {
                    "string" => StandardTokenType::String,
                    "other" => StandardTokenType::Other,
                    "comment" => StandardTokenType::Comment,
                    "regex" => StandardTokenType::RegEx,
                    _ => return None,
                };
                Some((selector, token_type))
            })
            .collect();
        let string_array = |key: &str, default: &[&str]| -> Vec<String> {
            match grammar[key].as_array() {
                Some(items) if items.iter().all(Value::is_string) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                _ => default.iter().map(|s| s.to_string()).collect(),
            }
        };
        for target in grammar["injectTo"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            self.injections.entry(target.to_string()).or_default().push(scope_name.to_string());
            self.injected_embedded_languages.entry(target.to_string()).or_default().extend(embedded_languages.iter().cloned());
        }
        if let Some(language) = grammar["language"].as_str() {
            self.language_to_scope.insert(language.to_string(), scope_name.to_string());
        }
        self.definitions.insert(
            scope_name.to_string(),
            Definition {
                path: location.join(path),
                embedded_languages,
                token_types,
                balanced_bracket_selectors: string_array("balancedBracketScopes", &["*"]),
                unbalanced_bracket_selectors: string_array("unbalancedBracketScopes", &[]),
            },
        );
    }

    pub fn has_grammar(&self, language: &str) -> bool {
        self.language_to_scope.contains_key(language)
    }

    /// `createGrammar`: the grammar of a language with its contribution's settings; `None` when no
    /// grammar is contributed for it or its file does not load.
    pub fn grammar(&self, language: &str) -> Option<Grammar> {
        let scope_name = self.language_to_scope.get(language)?;
        let definition = self.definitions.get(scope_name)?;
        let mut embedded_languages = definition.embedded_languages.clone();
        for (scope, embedded) in self.injected_embedded_languages.get(scope_name).into_iter().flatten() {
            match embedded_languages.iter_mut().find(|(existing, _)| existing == scope) {
                Some(entry) => entry.1 = embedded.clone(),
                None => embedded_languages.push((scope.clone(), embedded.clone())),
            }
        }
        let configuration = GrammarConfiguration {
            language: Some(language.to_string()),
            embedded_languages,
            token_types: definition.token_types.clone(),
            balanced_bracket_selectors: definition.balanced_bracket_selectors.clone(),
            unbalanced_bracket_selectors: definition.unbalanced_bracket_selectors.clone(),
        };
        let raw = self.raw_grammar(scope_name, None)?;
        Some(Grammar::new(raw, &configuration, self))
    }

    fn source(&self, scope_name: &str) -> Option<Rc<RawGrammarSource>> {
        if let Some(source) = self.sources.borrow().get(scope_name) {
            return source.clone();
        }
        let source = self.definitions.get(scope_name).and_then(|definition| {
            let loaded = std::fs::read_to_string(&definition.path).map_err(|e| e.to_string()).and_then(|content| RawGrammarSource::parse(&content, &definition.path.to_string_lossy()));
            match loaded {
                Ok(source) => Some(Rc::new(source)),
                Err(error) => {
                    eprintln!("Unable to load and parse grammar for scope {scope_name} from {}: {error}", definition.path.display());
                    None
                }
            }
        });
        self.sources.borrow_mut().insert(scope_name.to_string(), source.clone());
        source
    }
}

impl GrammarLookup for Registry {
    fn raw_grammar(&self, scope_name: &str, base: Option<Rc<RawRule>>) -> Option<RawGrammar> {
        Some(self.source(scope_name)?.instantiate(base))
    }

    /// Grammars injected into the scope or any of its dot prefixes.
    fn injections(&self, scope_name: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut end = 0;
        while end < scope_name.len() {
            end = scope_name[end + 1..].find('.').map_or(scope_name.len(), |i| end + 1 + i);
            result.extend(self.injections.get(&scope_name[..end]).into_iter().flatten().cloned());
        }
        result
    }
}
