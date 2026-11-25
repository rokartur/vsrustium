//! The providers extensions register through `MainThreadLanguageFeatures` (mainThreadLanguageFeatures.ts) and the
//! registry that orders them for a model (languageFeatureRegistry.ts, languageSelector.ts).

use serde_json::Value;

#[cfg(test)]
mod tests;

/// One `$register*` call: its method names the kind, its arguments start with the handle and, for providers
/// scored per model, the document selector.
pub struct Registration {
    pub handle: u64,
    pub method: String,
    pub args: Vec<Value>,
    time: u64,
}

impl Registration {
    pub fn selector(&self) -> &Value {
        self.args.get(1).unwrap_or(&Value::Null)
    }
}

#[derive(Default)]
pub struct LanguageFeatures {
    registrations: Vec<Registration>,
    clock: u64,
}

/// The model a registry orders providers for: `ITextModel.uri` as `UriComponents`, its language and whether it
/// is synchronized with the extension host (`shouldSynchronizeModel`).
pub struct Candidate<'a> {
    pub uri: &'a Value,
    pub language: &'a str,
}

impl LanguageFeatures {
    pub fn register(&mut self, method: &str, args: Vec<Value>) {
        let Some(handle) = args.first().and_then(Value::as_u64) else { return };
        self.clock += 1;
        self.registrations.push(Registration { handle, method: method.to_owned(), args, time: self.clock });
    }

    pub fn unregister(&mut self, handle: u64) {
        self.registrations.retain(|registration| registration.handle != handle);
    }

    /// The providers registered with `method` in registration order, for registries without a selector
    /// (`WorkspaceSymbolProviderRegistry`).
    pub fn all<'a>(&'a self, method: &'a str) -> impl Iterator<Item = &'a Registration> {
        self.registrations.iter().filter(move |registration| registration.method == method)
    }

    /// `LanguageFeatureRegistry.ordered`: the providers registered with `method` whose selector matches, best score
    /// first, then extension providers before built-in ones, then the newest; an exclusive selector that matches
    /// takes the model alone.
    pub fn ordered(&self, method: &str, candidate: &Candidate) -> Vec<&Registration> {
        let mut scored: Vec<(u32, &Registration)> = Vec::new();
        for registration in self.registrations.iter().filter(|registration| registration.method == method) {
            let value = score(registration.selector(), candidate.uri, candidate.language, true);
            if value > 0 && is_exclusive(registration.selector()) {
                return vec![registration];
            }
            if value > 0 {
                scored.push((value, registration));
            }
        }
        scored.sort_by(|(a_score, a), (b_score, b)| {
            b_score.cmp(a_score).then(is_builtin(a.selector()).cmp(&is_builtin(b.selector()))).then(b.time.cmp(&a.time))
        });
        scored.into_iter().map(|(_, registration)| registration).collect()
    }
}

/// languageSelector.ts `score` without notebooks: the port opens no notebook documents, so a filter with a
/// `notebookType` never matches.
pub fn score(selector: &Value, uri: &Value, language: &str, is_synchronized: bool) -> u32 {
    match selector {
        Value::Array(filters) => {
            let mut best = 0;
            for filter in filters {
                let value = score(filter, uri, language, is_synchronized);
                if value == 10 {
                    return value;
                }
                best = best.max(value);
            }
            best
        }
        Value::String(_) if !is_synchronized => 0,
        Value::String(selector) if selector == "*" => 5,
        Value::String(selector) if selector == language => 10,
        Value::String(_) => 0,
        Value::Object(filter) => {
            if !is_synchronized && filter.get("hasAccessToAllModels") != Some(&Value::Bool(true)) {
                return 0;
            }
            let mut value = 0;
            if let Some(scheme) = filter.get("scheme").and_then(Value::as_str).filter(|scheme| !scheme.is_empty()) {
                value = match scheme {
                    _ if scheme == uri["scheme"] => 10,
                    "*" => 5,
                    _ => return 0,
                };
            }
            if let Some(filter_language) = filter.get("language").and_then(Value::as_str).filter(|language| !language.is_empty()) {
                value = match filter_language {
                    _ if filter_language == language => 10,
                    "*" => value.max(5),
                    _ => return 0,
                };
            }
            if filter.get("notebookType").and_then(Value::as_str).is_some_and(|notebook_type| !notebook_type.is_empty()) {
                return 0;
            }
            if let Some(pattern) = filter.get("pattern").filter(|pattern| pattern.is_object() || pattern.as_str().is_some_and(|pattern| !pattern.is_empty())) {
                if !matches_pattern(pattern, &fs_path(uri)) {
                    return 0;
                }
                value = 10;
            }
            value
        }
        _ => 0,
    }
}

/// glob.ts `match` of a pattern string or an `IRelativePattern` against a file system path.
fn matches_pattern(pattern: &Value, path: &str) -> bool {
    let options = files::glob::Options::default();
    if let Some(pattern) = pattern.as_str() {
        return pattern == path || files::glob::match_pattern(pattern, path, options);
    }
    let base = pattern["baseUri"].as_object().map(|_| fs_path(&pattern["baseUri"])).or_else(|| pattern["base"].as_str().map(String::from)).unwrap_or_default();
    let base = base.trim_end_matches('/');
    let Some(relative) = path.strip_prefix(base).and_then(|rest| rest.strip_prefix('/')) else { return false };
    files::glob::match_pattern(pattern["pattern"].as_str().unwrap_or_default(), relative, options)
}

/// `URI.fsPath` on macOS: the path, with `//authority` first for a UNC-style URI.
fn fs_path(uri: &Value) -> String {
    let path = uri["path"].as_str().unwrap_or_default();
    match uri["authority"].as_str().filter(|authority| !authority.is_empty() && uri["scheme"] == "file") {
        Some(authority) => format!("//{authority}{path}"),
        None => path.to_owned(),
    }
}

fn is_exclusive(selector: &Value) -> bool {
    match selector {
        Value::Array(filters) => filters.iter().all(is_exclusive),
        Value::Object(filter) => filter.get("exclusive") == Some(&Value::Bool(true)),
        _ => false,
    }
}

fn is_builtin(selector: &Value) -> bool {
    match selector {
        Value::Array(filters) => filters.iter().any(is_builtin),
        Value::Object(filter) => filter.get("isBuiltin") == Some(&Value::Bool(true)),
        _ => false,
    }
}
