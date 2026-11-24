//! platform/configuration/common/configurationModels.ts and the value tree helpers of configuration.ts: one
//! layer of settings (`ConfigurationModel`), its parser, and the comparison that names the keys a change touched.

use serde_json::{Map, Value};

use crate::json::{self, ParseError, ParseOptions};
use crate::registry::{Registry, Scope};

/// `IOverrides`: the settings of one `[lang]` (or `[lang1][lang2]`) block.
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub identifiers: Vec<String>,
    pub keys: Vec<String>,
    pub contents: Map<String, Value>,
}

/// The unparsed input of a model, kept when parse options dropped some of it (`_raw`).
#[derive(Clone, Debug)]
enum Raw {
    Dict(Map<String, Value>),
    Model(ConfigurationModel),
}

#[derive(Clone, Debug, Default)]
pub struct ConfigurationModel {
    pub contents: Map<String, Value>,
    pub keys: Vec<String>,
    pub overrides: Vec<Override>,
    raw: Option<Vec<Raw>>,
}

/// `IConfigurationChange`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub keys: Vec<String>,
    pub overrides: Vec<(String, Vec<String>)>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.overrides.is_empty()
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({ "keys": self.keys, "overrides": self.overrides })
    }
}

/// `mergeChanges`.
pub fn merge_changes(changes: &[Change]) -> Change {
    let mut merged = Change::default();
    for change in changes {
        push_distinct(&mut merged.keys, &change.keys);
        for (identifier, keys) in &change.overrides {
            match merged.overrides.iter_mut().find(|(existing, _)| existing == identifier) {
                Some((_, existing)) => push_distinct(existing, keys),
                None => merged.overrides.push((identifier.clone(), distinct(keys))),
            }
        }
    }
    merged
}

impl ConfigurationModel {
    pub fn new(contents: Map<String, Value>, keys: Vec<String>, overrides: Vec<Override>) -> Self {
        ConfigurationModel { contents, keys, overrides, raw: None }
    }

    /// The model as `IConfigurationModel` JSON, the shape `$initializeConfiguration` carries.
    pub fn to_json(&self) -> Value {
        let overrides: Vec<Value> = self
            .overrides
            .iter()
            .map(|o| serde_json::json!({ "contents": o.contents, "identifiers": o.identifiers, "keys": o.keys }))
            .collect();
        serde_json::json!({ "contents": self.contents, "keys": self.keys, "overrides": overrides })
    }

    /// `toData()` of a user layer: also the raw input when parse options dropped part of it.
    pub fn to_json_with_raw(&self) -> Value {
        let mut value = self.to_json();
        if let Some([Raw::Dict(raw)]) = self.raw.as_deref() {
            value["raw"] = Value::Object(raw.clone());
        }
        value
    }

    /// `rawConfiguration`: the model of everything the input held, before parse options filtered it.
    pub fn raw_configuration(&self) -> ConfigurationModel {
        let Some(raws) = &self.raw else { return self.clone() };
        let models: Vec<ConfigurationModel> = raws
            .iter()
            .map(|raw| match raw {
                Raw::Model(model) => model.clone(),
                Raw::Dict(raw) => parse_raw(raw.clone(), None, &ParseFilter::default()).0,
            })
            .collect();
        let (first, rest) = models.split_first().expect("raw holds at least one model");
        first.merge(rest)
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.contents.is_empty() && self.overrides.is_empty()
    }

    /// `getValue(section)`; `None` section is the whole tree.
    pub fn get_value(&self, section: Option<&str>) -> Option<Value> {
        match section {
            Some(section) => get_configuration_value(&self.contents, section).cloned(),
            None => Some(Value::Object(self.contents.clone())),
        }
    }

    pub fn get_override_value(&self, section: Option<&str>, identifier: &str) -> Option<Value> {
        let contents = self.contents_for_override_identifier(identifier)?;
        match section {
            Some(section) => get_configuration_value(&contents, section).cloned(),
            None => Some(Value::Object(contents)),
        }
    }

    pub fn keys_for_override_identifier(&self, identifier: &str) -> Vec<String> {
        let mut keys = Vec::new();
        for o in self.overrides.iter().filter(|o| o.identifiers.iter().any(|i| i == identifier)) {
            push_distinct(&mut keys, &o.keys);
        }
        keys
    }

    pub fn all_override_identifiers(&self) -> Vec<String> {
        let mut identifiers = Vec::new();
        for o in &self.overrides {
            push_distinct(&mut identifiers, &o.identifiers);
        }
        identifiers
    }

    /// `override(identifier)`: the contents with the `[identifier]` block merged over them.
    pub fn override_for(&self, identifier: &str) -> ConfigurationModel {
        let Some(override_contents) = self.contents_for_override_identifier(identifier).filter(|c| !c.is_empty()) else {
            return self.clone();
        };
        let mut contents = Map::new();
        let mut keys: Vec<String> = self.contents.keys().cloned().collect();
        push_distinct(&mut keys, &override_contents.keys().cloned().collect::<Vec<_>>());
        for key in keys {
            let mut value = self.contents.get(&key).cloned();
            if let Some(override_value) = override_contents.get(&key).filter(|v| is_truthy(v)) {
                value = match (value, override_value) {
                    (Some(Value::Object(mut base)), Value::Object(add)) => {
                        merge_contents(&mut base, add);
                        Some(Value::Object(base))
                    }
                    _ => Some(override_value.clone()),
                };
            }
            if let Some(value) = value {
                contents.insert(key, value);
            }
        }
        ConfigurationModel { contents, keys: self.keys.clone(), overrides: self.overrides.clone(), raw: None }
    }

    /// `merge(...others)`.
    pub fn merge(&self, others: &[ConfigurationModel]) -> ConfigurationModel {
        let mut contents = self.contents.clone();
        let mut overrides = self.overrides.clone();
        let mut keys = self.keys.clone();
        let mut raws = self.raws_or_self();
        for other in others {
            raws.extend(other.raws_or_self());
            if other.is_empty() {
                continue;
            }
            merge_contents(&mut contents, &other.contents);
            for other_override in &other.overrides {
                match overrides.iter_mut().find(|o| o.identifiers == other_override.identifiers) {
                    Some(o) => {
                        merge_contents(&mut o.contents, &other_override.contents);
                        push_distinct(&mut o.keys, &other_override.keys);
                    }
                    None => overrides.push(other_override.clone()),
                }
            }
            push_distinct(&mut keys, &other.keys);
        }
        let raw = (!raws.iter().all(|raw| matches!(raw, Raw::Model(_)))).then_some(raws);
        ConfigurationModel { contents, keys, overrides, raw }
    }

    fn raws_or_self(&self) -> Vec<Raw> {
        match &self.raw {
            Some(raws) => raws.clone(),
            None => vec![Raw::Model(ConfigurationModel { raw: None, ..self.clone() })],
        }
    }

    fn contents_for_override_identifier(&self, identifier: &str) -> Option<Map<String, Value>> {
        let mut identifier_only = None;
        let mut contents: Option<Map<String, Value>> = None;
        let mut merge = |add: &Map<String, Value>| match &mut contents {
            Some(contents) => merge_contents(contents, add),
            None => contents = Some(add.clone()),
        };
        for o in &self.overrides {
            if o.identifiers.len() == 1 && o.identifiers[0] == identifier {
                identifier_only = Some(&o.contents);
            } else if o.identifiers.iter().any(|i| i == identifier) {
                merge(&o.contents);
            }
        }
        // The block for the identifier alone goes last so it wins over `[a][b]` blocks.
        if let Some(only) = identifier_only {
            merge(only);
        }
        contents
    }

    /// `setValue(key, value)`.
    pub fn set_value(&mut self, key: &str, value: Value) {
        add_to_value_tree(&mut self.contents, key, value, &mut |_| {});
        if !self.keys.iter().any(|k| k == key) {
            self.keys.push(key.to_owned());
        }
        if is_override_key(key) {
            let override_contents = self.contents[key].as_object().cloned().unwrap_or_default();
            let identifiers = override_identifiers_from_key(key);
            let o = Override {
                identifiers: identifiers.clone(),
                keys: override_contents.keys().cloned().collect(),
                contents: to_values_tree(&override_contents, &mut |_| {}),
            };
            match self.overrides.iter().position(|o| o.identifiers == identifiers) {
                Some(index) => self.overrides[index] = o,
                None => self.overrides.push(o),
            }
        }
    }

    /// `removeValue(key)`.
    pub fn remove_value(&mut self, key: &str) {
        let Some(index) = self.keys.iter().position(|k| k == key) else { return };
        self.keys.remove(index);
        remove_from_value_tree(&mut self.contents, key);
        if is_override_key(key) {
            let identifiers = override_identifiers_from_key(key);
            self.overrides.retain(|o| o.identifiers != identifiers);
        }
    }
}

/// `ConfigurationParseOptions`.
#[derive(Clone, Debug, Default)]
pub struct ParseFilter {
    pub scopes: Option<Vec<Scope>>,
    pub skip_restricted: bool,
    pub skip_unregistered: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl ParseFilter {
    fn filters(&self) -> bool {
        self.scopes.is_some() || self.skip_restricted || self.skip_unregistered || !self.exclude.is_empty()
    }
}

/// What `ConfigurationModelParser.parse` leaves: the model, the restricted keys it saw and the JSON errors.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub model: ConfigurationModel,
    pub restricted: Vec<String>,
    pub errors: Vec<ParseError>,
}

/// `ConfigurationModelParser.parse(content, options)` of a settings file. A root that is not an object reads as `{}`.
pub fn parse_content(content: &str, registry: Option<&Registry>, filter: &ParseFilter) -> Parsed {
    let mut errors = Vec::new();
    let raw = match json::parse(content, &mut errors, ParseOptions::default()) {
        Some(Value::Object(raw)) => raw,
        _ => Map::new(),
    };
    let (model, restricted) = parse_raw(raw, registry, filter);
    Parsed { model, restricted, errors }
}

/// `ConfigurationModelParser.parseRaw(raw, options)`.
pub fn parse_raw(raw: Map<String, Value>, registry: Option<&Registry>, filter: &ParseFilter) -> (ConfigurationModel, Vec<String>) {
    let (filtered, restricted, has_excluded) = match registry.filter(|_| filter.filters()) {
        Some(registry) => filter_properties(&raw, registry, true, filter),
        None => (raw.clone(), Vec::new(), false),
    };
    let contents = to_values_tree(&filtered, &mut |_| {});
    let keys = filtered.keys().cloned().collect();
    let overrides = filtered
        .iter()
        .filter(|(key, _)| is_override_key(key))
        .map(|(key, value)| {
            let block = value.as_object().cloned().unwrap_or_default();
            Override { identifiers: override_identifiers_from_key(key), keys: block.keys().cloned().collect(), contents: to_values_tree(&block, &mut |_| {}) }
        })
        .collect();
    let raw = has_excluded.then(|| vec![Raw::Dict(raw)]);
    (ConfigurationModel { contents, keys, overrides, raw }, restricted)
}

struct Filtered {
    included: bool,
    value: Value,
    restricted: Vec<String>,
    has_excluded: bool,
}

fn filter_properties(properties: &Map<String, Value>, registry: &Registry, filter_overridden: bool, filter: &ParseFilter) -> (Map<String, Value>, Vec<String>, bool) {
    let mut raw = Map::new();
    let mut restricted = Vec::new();
    let mut has_excluded = false;
    for (key, value) in properties {
        if filter_overridden && is_override_key(key) {
            let (nested, nested_restricted, nested_excluded) = filter_properties(value.as_object().unwrap_or(&Map::new()), registry, false, filter);
            raw.insert(key.clone(), Value::Object(nested));
            has_excluded |= nested_excluded;
            restricted.extend(nested_restricted);
            continue;
        }
        let result = filter_property(key, value, registry, filter);
        restricted.extend(result.restricted);
        has_excluded |= result.has_excluded;
        if result.included {
            raw.insert(key.clone(), result.value);
        } else {
            has_excluded = true;
        }
    }
    (raw, restricted, has_excluded)
}

fn filter_property(path: &str, value: &Value, registry: &Registry, filter: &ParseFilter) -> Filtered {
    let leaf = |included| Filtered { included, value: value.clone(), restricted: Vec::new(), has_excluded: false };
    if let Some(property) = registry.property(path) {
        let restricted = if property.restricted { vec![path.to_owned()] } else { Vec::new() };
        return Filtered { restricted, ..leaf(should_include(path, Some(property.scope), property.restricted, true, filter)) };
    }
    let object = value.as_object().filter(|object| !object.is_empty());
    let Some(object) = object.filter(|_| !filter.exclude.iter().any(|e| e == path)) else {
        if value.is_object() && filter.exclude.iter().any(|e| e == path) {
            return leaf(false);
        }
        return filter_unregistered_leaf(path, value, registry, filter);
    };
    let mut nested = Map::new();
    let mut restricted = Vec::new();
    let mut included = false;
    let mut has_excluded = false;
    for (key, child) in object {
        let result = filter_property(&format!("{path}.{key}"), child, registry, filter);
        restricted.extend(result.restricted);
        has_excluded |= result.has_excluded;
        if result.included {
            nested.insert(key.clone(), result.value);
            included = true;
        } else {
            has_excluded = true;
        }
    }
    Filtered { included, value: Value::Object(nested), restricted, has_excluded }
}

/// A value under no registered key counts as part of its nearest registered ancestor (`findRegisteredAncestor`).
fn filter_unregistered_leaf(path: &str, value: &Value, registry: &Registry, filter: &ParseFilter) -> Filtered {
    let mut end = path.rfind('.');
    while let Some(index) = end.filter(|index| *index > 0) {
        let ancestor = &path[..index];
        if let Some(property) = registry.property(ancestor) {
            let restricted = if property.restricted { vec![ancestor.to_owned()] } else { Vec::new() };
            return Filtered { included: should_include(ancestor, Some(property.scope), property.restricted, true, filter), value: value.clone(), restricted, has_excluded: false };
        }
        end = ancestor.rfind('.');
    }
    Filtered { included: should_include(path, None, false, false, filter), value: value.clone(), restricted: Vec::new(), has_excluded: false }
}

fn should_include(key: &str, scope: Option<Scope>, restricted: bool, registered: bool, filter: &ParseFilter) -> bool {
    if filter.exclude.iter().any(|e| e == key) {
        return false;
    }
    if filter.include.iter().any(|i| i == key) {
        return true;
    }
    if filter.skip_restricted && restricted {
        return false;
    }
    if filter.skip_unregistered && !registered {
        return false;
    }
    match (scope, &filter.scopes) {
        (Some(scope), Some(scopes)) => scopes.contains(&scope),
        _ => true,
    }
}

/// `compare(from, to)` of two models: added, updated and removed keys, and the keys each override changed.
pub fn compare(from: Option<&ConfigurationModel>, to: Option<&ConfigurationModel>) -> Change {
    let from_raw = from.map(ConfigurationModel::raw_configuration);
    let to_raw = to.map(ConfigurationModel::raw_configuration);
    let (added, removed, updated) = compare_contents(to_raw.as_ref().map(|m| (&m.keys, &m.contents)), from_raw.as_ref().map(|m| (&m.keys, &m.contents)));
    let mut overrides = Vec::new();
    let from_identifiers = from.map(ConfigurationModel::all_override_identifiers).unwrap_or_default();
    let to_identifiers = to.map(ConfigurationModel::all_override_identifiers).unwrap_or_default();
    if let Some(to) = to {
        for identifier in to_identifiers.iter().filter(|i| !from_identifiers.contains(i)) {
            overrides.push((identifier.clone(), to.keys_for_override_identifier(identifier)));
        }
    }
    if let Some(from) = from {
        for identifier in from_identifiers.iter().filter(|i| !to_identifiers.contains(i)) {
            overrides.push((identifier.clone(), from.keys_for_override_identifier(identifier)));
        }
    }
    if let (Some(from), Some(to)) = (from, to) {
        for identifier in from_identifiers.iter().filter(|i| to_identifiers.contains(i)) {
            let from_contents = from.get_override_value(None, identifier).and_then(|v| v.as_object().cloned()).unwrap_or_default();
            let to_contents = to.get_override_value(None, identifier).and_then(|v| v.as_object().cloned()).unwrap_or_default();
            let from_keys = from.keys_for_override_identifier(identifier);
            let to_keys = to.keys_for_override_identifier(identifier);
            let (added, removed, updated) = compare_contents(Some((&to_keys, &to_contents)), Some((&from_keys, &from_contents)));
            overrides.push((identifier.clone(), [added, removed, updated].concat()));
        }
    }
    Change { keys: [added, updated, removed].concat(), overrides }
}

type KeysAndContents<'a> = (&'a Vec<String>, &'a Map<String, Value>);

fn compare_contents(to: Option<KeysAndContents>, from: Option<KeysAndContents>) -> (Vec<String>, Vec<String>, Vec<String>) {
    let added = match (to, from) {
        (Some(to), Some(from)) => to.0.iter().filter(|k| !from.0.contains(k)).cloned().collect(),
        (Some(to), None) => to.0.clone(),
        _ => Vec::new(),
    };
    let removed = match (from, to) {
        (Some(from), Some(to)) => from.0.iter().filter(|k| !to.0.contains(k)).cloned().collect(),
        (Some(from), None) => from.0.clone(),
        _ => Vec::new(),
    };
    let mut updated = Vec::new();
    if let (Some(to), Some(from)) = (to, from) {
        for key in from.0.iter().filter(|k| to.0.contains(k)) {
            if get_configuration_value(from.1, key) != get_configuration_value(to.1, key) {
                updated.push(key.clone());
            }
        }
    }
    (added, removed, updated)
}

/// `OVERRIDE_PROPERTY_REGEX`: `^(\[([^\]]+)\])+$`.
pub fn is_override_key(key: &str) -> bool {
    let mut rest = key;
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let Some(inner) = rest.strip_prefix('[') else { return false };
        let Some(end) = inner.find(']') else { return false };
        if end == 0 {
            return false;
        }
        rest = &inner[end + 1..];
    }
    true
}

/// `overrideIdentifiersFromKey`: the trimmed, distinct identifiers of `[a][b]`.
pub fn override_identifiers_from_key(key: &str) -> Vec<String> {
    if !is_override_key(key) {
        return Vec::new();
    }
    let identifiers: Vec<String> = key[1..key.len() - 1].split("][").map(str::trim).filter(|i| !i.is_empty()).map(str::to_owned).collect();
    distinct(&identifiers)
}

/// `keyFromOverrideIdentifiers`.
pub fn key_from_override_identifiers(identifiers: &[String]) -> String {
    identifiers.iter().map(|i| format!("[{i}]")).collect()
}

/// `getConfigurationValue(config, 'a.b.c')`.
pub fn get_configuration_value<'v>(config: &'v Map<String, Value>, path: &str) -> Option<&'v Value> {
    let mut segments = path.split('.');
    let mut current = config.get(segments.next()?)?;
    for segment in segments {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// `toValuesTree`: dotted keys become nested objects.
pub fn to_values_tree(properties: &Map<String, Value>, conflict: &mut dyn FnMut(String)) -> Map<String, Value> {
    let mut root = Map::new();
    for (key, value) in properties {
        add_to_value_tree(&mut root, key, value.clone(), conflict);
    }
    root
}

/// `addToValueTree`.
pub fn add_to_value_tree(root: &mut Map<String, Value>, key: &str, value: Value, conflict: &mut dyn FnMut(String)) {
    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments.split_last().expect("split yields one segment");
    let mut current = root;
    for (index, segment) in parents.iter().enumerate() {
        let child = current.entry(segment.to_string()).or_insert_with(|| Value::Object(Map::new()));
        match child {
            Value::Object(map) => current = map,
            other => {
                let at = parents[..=index].join(".");
                let found = if other.is_null() { "null".to_owned() } else { other.to_string() };
                conflict(format!("Ignoring {key} as {at} is {found}"));
                return;
            }
        }
    }
    current.insert(last.to_string(), value);
}

/// `removeFromValueTree`: also drops parents left empty.
pub fn remove_from_value_tree(tree: &mut Map<String, Value>, key: &str) {
    let segments: Vec<&str> = key.split('.').collect();
    remove_segments(tree, &segments);
}

fn remove_segments(tree: &mut Map<String, Value>, segments: &[&str]) {
    let (first, rest) = segments.split_first().expect("non-empty path");
    if rest.is_empty() {
        tree.shift_remove(*first);
        return;
    }
    if let Some(Value::Object(child)) = tree.get_mut(*first) {
        remove_segments(child, rest);
        if child.is_empty() {
            tree.shift_remove(*first);
        }
    }
}

/// `mergeContents`: objects merge key by key, anything else from `add` replaces.
pub fn merge_contents(base: &mut Map<String, Value>, add: &Map<String, Value>) {
    for (key, value) in add {
        if let (Some(Value::Object(base_child)), Value::Object(add_child)) = (base.get_mut(key), value) {
            merge_contents(base_child, add_child);
            continue;
        }
        base.insert(key.clone(), value.clone());
    }
}

/// JavaScript truthiness, which `createOverrideConfigurationModel` applies to override values.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

pub(crate) fn distinct(items: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    push_distinct(&mut result, items);
    result
}

pub(crate) fn push_distinct(target: &mut Vec<String>, items: &[String]) {
    for item in items {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}

/// platform/configuration/test/common/configuration.test.ts and the ConfigurationModelParser and ConfigurationModel
/// suites of configurationModels.test.ts. Left out: `inspect` of a model (the port inspects a `Configuration`),
/// `objects.merge` with overwrite off (unused here) and the `__proto__` cases (a serde map has no prototype).
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn model(contents: Value, keys: &[&str], overrides: Vec<Override>) -> ConfigurationModel {
        ConfigurationModel::new(map(contents), strings(keys), overrides)
    }

    fn o(identifiers: &[&str], contents: Value, keys: &[&str]) -> Override {
        Override { identifiers: strings(identifiers), keys: strings(keys), contents: map(contents) }
    }

    fn removed(mut tree: Value, key: &str) -> Value {
        remove_from_value_tree(tree.as_object_mut().unwrap(), key);
        tree
    }

    fn change(keys: &[&str], overrides: &[(&str, &[&str])]) -> Change {
        Change { keys: strings(keys), overrides: overrides.iter().map(|(identifier, keys)| (identifier.to_string(), strings(keys))).collect() }
    }

    fn parse(content: Value, filter: ParseFilter) -> ConfigurationModel {
        let mut registry = Registry::default();
        let contribution = json!({ "contributes": { "configuration": { "id": "ConfigurationModelParserTest", "type": "object", "properties": {
            "ConfigurationModelParserTest.windowSetting": { "type": "string", "default": "isSet" }
        } } } });
        registry.register_configuration("test", &contribution);
        registry.finish();
        parse_content(&content.to_string(), Some(&registry), &filter).model
    }

    #[test]
    fn object_merge() {
        let mut base = map(json!({ "a": { "b": 1, "c": true, "d": 2 } }));
        merge_contents(&mut base, &map(json!({ "a": { "c": false, "e": "a" } })));
        assert_eq!(Value::Object(base), json!({ "a": { "b": 1, "c": false, "d": 2, "e": "a" } }));
        let mut base = map(json!({ "a": ["b", "c"] }));
        merge_contents(&mut base, &map(json!({ "a": ["b", "d"] })));
        assert_eq!(Value::Object(base), json!({ "a": ["b", "d"] }));
    }

    #[test]
    fn remove_from_value_tree_cases() {
        assert_eq!(removed(json!({ "a": { "b": 2 } }), "c"), json!({ "a": { "b": 2 } }));
        assert_eq!(removed(json!({ "a": { "b": 2 } }), "a.b.c"), json!({ "a": { "b": 2 } }));
        assert_eq!(removed(json!({ "a": 1 }), "a"), json!({}));
        assert_eq!(removed(json!({ "a": null }), "a"), json!({}));
        assert_eq!(removed(json!({ "a": { "b": 1 } }), "a.b"), json!({}));
        assert_eq!(removed(json!({ "a": { "b": [1] } }), "a.b"), json!({}));
        assert_eq!(removed(json!({ "a": [1] }), "a.0"), json!({ "a": [1] }));
        assert_eq!(removed(json!({ "a": { "b": 1 } }), "a"), json!({}));
        assert_eq!(removed(json!({ "a": { "b": { "c": 1 }, "d": 1 } }), "a.b.c"), json!({ "a": { "d": 1 } }));
        assert_eq!(removed(json!({ "a": { "b": { "c": { "d": 1 }, "d": 1 } } }), "a.b.c.d"), json!({ "a": { "b": { "d": 1 } } }));
        assert_eq!(removed(json!({ "a": { "b": { "c": 1, "d": 1 } } }), "a.b.c"), json!({ "a": { "b": { "d": 1 } } }));
    }

    #[test]
    fn merge_changes_cases() {
        assert_eq!(merge_changes(&[change(&["a", "b"], &[]), change(&["c", "d"], &[])]), change(&["a", "b", "c", "d"], &[]));
        assert_eq!(merge_changes(&[change(&["a", "b"], &[]), change(&["c", "d"], &[]), change(&["a", "d", "e"], &[])]), change(&["a", "b", "c", "d", "e"], &[]));
        assert_eq!(merge_changes(&[change(&[], &[("a", &["1", "2"])]), change(&[], &[("b", &["3", "4"])])]), change(&[], &[("a", &["1", "2"]), ("b", &["3", "4"])]));
        assert_eq!(
            merge_changes(&[change(&[], &[("a", &["1", "2"]), ("b", &["5", "4"])]), change(&[], &[("b", &["3", "4"])]), change(&[], &[("c", &["1", "4"]), ("a", &["2", "3"])])]),
            change(&[], &[("a", &["1", "2", "3"]), ("b", &["5", "4", "3"]), ("c", &["1", "4"])])
        );
        assert_eq!(
            merge_changes(&[change(&["b", "b"], &[("a", &["1", "2"]), ("b", &["5", "4"])]), change(&["b"], &[("b", &["3", "4"])]), change(&["c", "a"], &[("c", &["1", "4"]), ("a", &["2", "3"])])]),
            change(&["b", "c", "a"], &[("a", &["1", "2", "3"]), ("b", &["5", "4", "3"]), ("c", &["1", "4"])])
        );
        assert_eq!(merge_changes(&[]), change(&[], &[]));
    }

    #[test]
    fn parser_override_identifiers() {
        assert_eq!(parse(json!({ "[x]": { "a": 1 } }), ParseFilter::default()).overrides, [o(&["x"], json!({ "a": 1 }), &["a"])]);
        assert_eq!(parse(json!({ "[x][y]": { "a": 1 } }), ParseFilter::default()).overrides, [o(&["x", "y"], json!({ "a": 1 }), &["a"])]);
        assert_eq!(parse(json!({ "[x][y][x][z]": { "a": 1 } }), ParseFilter::default()).overrides, [o(&["x", "y", "z"], json!({ "a": 1 }), &["a"])]);
    }

    #[test]
    fn parser_filters() {
        let parsed = parse(json!({ "a": 1, "b": 2 }), ParseFilter { exclude: strings(&["a"]), ..ParseFilter::default() });
        assert_eq!((parsed.get_value(Some("a")), parsed.get_value(Some("b"))), (None, Some(json!(2))));
        let parsed = parse(json!({ "a": 1, "b": 2 }), ParseFilter { exclude: strings(&["a"]), include: strings(&["a"]), ..ParseFilter::default() });
        assert_eq!((parsed.get_value(Some("a")), parsed.get_value(Some("b"))), (None, Some(json!(2))));
        let window_setting = json!({ "ConfigurationModelParserTest.windowSetting": "1" });
        let parsed = parse(window_setting.clone(), ParseFilter { scopes: Some(vec![Scope::Application]), ..ParseFilter::default() });
        assert_eq!(parsed.get_value(Some("ConfigurationModelParserTest.windowSetting")), None);
        let parsed = parse(window_setting, ParseFilter { scopes: Some(vec![Scope::Application]), include: strings(&["ConfigurationModelParserTest.windowSetting"]), ..ParseFilter::default() });
        assert_eq!(parsed.get_value(Some("ConfigurationModelParserTest.windowSetting")), Some(json!("1")));
        let parsed = parse(json!({ "constructor": true, "toString": true }), ParseFilter { skip_unregistered: true, ..ParseFilter::default() });
        assert_eq!((parsed.get_value(Some("constructor")), parsed.get_value(Some("toString"))), (None, None));
    }

    #[test]
    fn parser_invalid_setting_key() {
        let parsed = parse(json!({ "a": null, "a.b.c": { "c": 1 } }), ParseFilter::default());
        assert_eq!((parsed.get_value(Some("a")), parsed.get_value(Some("a.b")), parsed.get_value(Some("a.b.c"))), (Some(json!(null)), None, None));
    }

    #[test]
    fn set_value() {
        let mut m = model(json!({ "a": { "b": 1 } }), &["a.b"], vec![]);
        m.set_value("f", json!(1));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 1 }, "f": 1 }), strings(&["a.b", "f"])));
        let mut m = model(json!({ "a": { "b": 1 }, "f": 1 }), &["a.b", "f"], vec![]);
        m.set_value("f", json!(3));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 1 }, "f": 3 }), strings(&["a.b", "f"])));
        let mut m = model(json!({ "a": { "b": 1 }, "f": 1 }), &["a.b", "f"], vec![]);
        m.set_value("b.c", json!(1));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 1 }, "f": 1, "b": { "c": 1 } }), strings(&["a.b", "f", "b.c"])));
        let mut m = model(json!({ "a": { "b": 1 }, "b": { "c": 1 }, "f": 1 }), &["a.b", "b.c", "f"], vec![]);
        m.set_value("b.c", json!(3));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 1 }, "b": { "c": 3 }, "f": 1 }), strings(&["a.b", "b.c", "f"])));
        let mut m = model(json!({ "a": { "b": 1 }, "f": 1 }), &["a.b", "f"], vec![]);
        m.set_value("a.c", json!(1));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 1, "c": 1 }, "f": 1 }), strings(&["a.b", "f", "a.c"])));
        let mut m = model(json!({ "a": { "b": 1, "c": 1 }, "f": 1 }), &["a.b", "a.c", "f"], vec![]);
        m.set_value("a.c", json!(3));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 1, "c": 3 }, "f": 1 }), strings(&["a.b", "a.c", "f"])));
        let mut m = model(json!({ "a": { "b": {} }, "f": 1 }), &["a.b", "f"], vec![]);
        m.set_value("a.b.c", json!(1));
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": { "c": 1 } }, "f": 1 }), strings(&["a.b", "f", "a.b.c"])));
    }

    #[test]
    fn remove_value() {
        let mut m = model(json!({ "a": { "b": 2 } }), &["a.b"], vec![]);
        m.remove_value("a.b.c");
        assert_eq!((Value::Object(m.contents), m.keys), (json!({ "a": { "b": 2 } }), strings(&["a.b"])));
        let mut m = model(json!({ "a": 1 }), &["a"], vec![]);
        m.remove_value("a");
        assert_eq!((Value::Object(m.contents), m.keys), (json!({}), strings(&[])));
        let mut m = model(json!({ "a": { "b": 1 } }), &["a.b"], vec![]);
        m.remove_value("a.b");
        assert_eq!((Value::Object(m.contents), m.keys), (json!({}), strings(&[])));
    }

    #[test]
    fn override_for() {
        let contents = |m: ConfigurationModel, identifier| Value::Object(m.override_for(identifier).contents);
        let base = json!({ "a": { "b": 1 }, "f": 1 });
        assert_eq!(contents(model(base.clone(), &[], vec![o(&["c"], json!({ "a": { "d": 1 } }), &["a"])]), "c"), json!({ "a": { "b": 1, "d": 1 }, "f": 1 }));
        assert_eq!(contents(model(base.clone(), &[], vec![o(&["c"], json!({ "a": { "d": 1 } }), &["a"])]), "xyz"), base);
        assert_eq!(contents(model(base.clone(), &[], vec![o(&["c"], json!({ "a": { "d": 1 }, "g": 1 }), &["a", "g"])]), "c"), json!({ "a": { "b": 1, "d": 1 }, "f": 1, "g": 1 }));
        assert_eq!(contents(model(base, &[], vec![o(&["c"], json!({ "a": { "d": 1 }, "f": { "g": 1 } }), &["a", "f"])]), "c"), json!({ "a": { "b": 1, "d": 1 }, "f": { "g": 1 } }));
        let base = json!({ "a": { "b": 1 }, "f": { "g": 1 } });
        assert_eq!(contents(model(base.clone(), &[], vec![o(&["c"], json!({ "a": { "d": 1 }, "f": 1 }), &["a", "f"])]), "c"), json!({ "a": { "b": 1, "d": 1 }, "f": 1 }));
        assert_eq!(contents(model(base.clone(), &[], vec![o(&["c"], json!({}), &[])]), "c"), base);
        assert_eq!(contents(model(json!({ "a": 1, "c": 1 }), &[], vec![o(&["b"], json!({ "a": 2 }), &["a"])]), "b"), json!({ "a": 2, "c": 1 }));
    }

    #[test]
    fn override_with_multiple_identifiers() {
        let m = model(json!({ "a": 1, "c": 1 }), &["a", "c"], vec![o(&["x", "y"], json!({ "a": 2 }), &["a"])]);
        for identifier in ["x", "y"] {
            let actual = m.override_for(identifier);
            assert_eq!((Value::Object(actual.contents), actual.keys), (json!({ "a": 2, "c": 1 }), strings(&["a", "c"])));
            assert_eq!(m.keys_for_override_identifier(identifier), strings(&["a"]));
        }
        let m = model(json!({ "a": 1, "c": 1 }), &["a", "c"], vec![o(&["x"], json!({ "a": 3, "b": 1 }), &["a", "b"]), o(&["x", "y"], json!({ "a": 2 }), &["a"])]);
        let actual = m.override_for("x");
        assert_eq!((Value::Object(actual.contents), actual.keys), (json!({ "a": 3, "c": 1, "b": 1 }), strings(&["a", "c"])));
        assert_eq!(m.keys_for_override_identifier("x"), strings(&["a", "b"]));
    }

    #[test]
    fn merge() {
        let result = model(json!({ "a": 1, "b": 2 }), &["a", "b"], vec![]).merge(&[model(json!({ "a": 3, "c": 4 }), &["a", "c"], vec![])]);
        assert_eq!((Value::Object(result.contents), result.keys), (json!({ "a": 3, "b": 2, "c": 4 }), strings(&["a", "b", "c"])));
        let result = model(json!({ "a": { "b": 1 } }), &["a.b"], vec![]).merge(&[model(json!({ "a": { "b": 2 } }), &["a.b"], vec![])]);
        assert_eq!(result.get_value(Some("a")), Some(json!({ "b": 2 })));
        assert_eq!(result.keys, strings(&["a.b"]));

        let base = model(json!({ "a": { "b": 1 } }), &["a.b"], vec![o(&["c"], json!({ "a": 2 }), &["a"])]);
        let result = base.merge(&[model(json!({ "a": { "b": 2 } }), &["a.b"], vec![o(&["c"], json!({ "b": 2 }), &["b"])])]);
        assert_eq!(Value::Object(result.contents.clone()), json!({ "a": { "b": 2 } }));
        assert_eq!(result.overrides, [o(&["c"], json!({ "a": 2, "b": 2 }), &["a", "b"])]);
        assert_eq!(Value::Object(result.override_for("c").contents), json!({ "a": 2, "b": 2 }));

        let base = model(json!({ "a": { "b": 1 }, "f": 1 }), &["a.b", "f"], vec![o(&["c"], json!({ "a": { "d": 1 } }), &["a"])]);
        let result = base.merge(&[model(json!({ "a": { "b": 2 } }), &["a.b"], vec![o(&["c"], json!({ "a": { "e": 2 } }), &["a"])])]);
        assert_eq!(Value::Object(result.contents.clone()), json!({ "a": { "b": 2 }, "f": 1 }));
        assert_eq!(result.overrides, [o(&["c"], json!({ "a": { "d": 1, "e": 2 } }), &["a"])]);
        assert_eq!(Value::Object(result.override_for("c").contents), json!({ "a": { "b": 2, "d": 1, "e": 2 }, "f": 1 }));
        assert_eq!(result.keys, strings(&["a.b", "f"]));

        let m = model(json!({ "a": 1, "c": 1 }), &["a", "c"], vec![o(&["y"], json!({ "c": 1 }), &["c"]), o(&["x", "y"], json!({ "a": 2 }), &["a"])]);
        let target = model(json!({ "a": 2, "b": 1 }), &["a", "b"], vec![o(&["x"], json!({ "a": 3, "b": 2 }), &["a", "b"]), o(&["x", "y"], json!({ "b": 3 }), &["b"])]);
        let actual = m.merge(&[target]);
        assert_eq!((Value::Object(actual.contents), actual.keys), (json!({ "a": 2, "c": 1, "b": 1 }), strings(&["a", "c", "b"])));
        assert_eq!(actual.overrides, [o(&["y"], json!({ "c": 1 }), &["c"]), o(&["x", "y"], json!({ "a": 2, "b": 3 }), &["a", "b"]), o(&["x"], json!({ "a": 3, "b": 2 }), &["a", "b"])]);
    }

    #[test]
    fn get_value() {
        assert_eq!(model(json!({ "a": 1 }), &[], vec![]).get_value(Some("a")), Some(json!(1)));
        assert_eq!(model(json!({ "a": { "b": 1 } }), &[], vec![]).get_value(Some("a")), Some(json!({ "b": 1 })));
        assert_eq!(model(json!({ "awesome": true }), &[], vec![]).get_value(Some("unknownproperty")), None);
    }
}
