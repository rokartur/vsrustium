//! Semantic token styling as VS Code 1.147.0 resolves it: the token classification registry
//! (platform/theme/common/tokenClassificationRegistry.ts), the semantic parts of `ColorThemeData`
//! (workbench/services/themes/common/colorThemeData.ts), `isSemanticColoringEnabled`
//! (editor/contrib/semanticTokens/common/semanticTokensConfig.ts), and in `provider_styling` the provider's reply
//! turned into styled ranges.
#![allow(dead_code, reason = "the editor integration calls this module next")]

mod provider_styling;
mod scope_matcher;

use std::collections::HashMap;

use serde_json::Value;

use crate::theme::ThemeKind;
use scope_matcher::{ScopeMatcher, create_matchers};

pub use provider_styling::{
    InvalidSemanticTokensEdit, SemanticTokensDto, SemanticTokensEdit, SemanticTokensLegend, SemanticTokensProviderStyling, StyledRange, apply_semantic_tokens_edits,
    decode_semantic_tokens_dto, to_multiline_tokens,
};

/// `TokenStyle`; `None` leaves the property to the TextMate style. The foreground is `#RRGGBB` or `#RRGGBBAA`
/// in upper case, as `TokenColorIndex` keeps it after `normalizeColor`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenStyle {
    pub foreground: Option<String>,
    pub bold: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub italic: Option<bool>,
}

impl TokenStyle {
    /// `TokenStyle.fromSettings`: a `fontStyle` sets each flag it does not name to false and wins over the booleans.
    pub fn from_settings(foreground: Option<&str>, font_style: Option<&str>, bold: Option<bool>, underline: Option<bool>, strikethrough: Option<bool>, italic: Option<bool>) -> TokenStyle {
        let mut style = TokenStyle { foreground: foreground.map(color_from_hex), bold, underline, strikethrough, italic };
        let Some(font_style) = font_style else { return style };
        (style.bold, style.underline, style.strikethrough, style.italic) = (Some(false), Some(false), Some(false), Some(false));
        // `/italic|bold|underline|strikethrough/g`: the words may touch, as in "bolditalic".
        let mut rest = font_style;
        while let Some(first) = rest.chars().next() {
            let word = ["italic", "bold", "underline", "strikethrough"].into_iter().find(|word| rest.starts_with(word));
            match word {
                Some("italic") => style.italic = Some(true),
                Some("bold") => style.bold = Some(true),
                Some("underline") => style.underline = Some(true),
                Some(_) => style.strikethrough = Some(true),
                None => {}
            }
            rest = &rest[word.map_or(first.len_utf8(), str::len)..];
        }
        style
    }
}

/// `Color.fromHex` (an unparsable color is red, a non-hex digit 0) printed by `Color.Format.CSS.formatHexA(color, true)`
/// in upper case.
fn color_from_hex(hex: &str) -> String {
    let units: Vec<u16> = hex.encode_utf16().collect();
    let digit = |i: usize| char::from_u32(units[i] as u32).and_then(|c| c.to_digit(16)).unwrap_or(0);
    let pair = |i: usize| 16 * digit(i) + digit(i + 1);
    let single = |i: usize| 17 * digit(i);
    let (red, green, blue, alpha) = match units.len() {
        _ if units.first() != Some(&(b'#' as u16)) => (255, 0, 0, 255),
        7 => (pair(1), pair(3), pair(5), 255),
        9 => (pair(1), pair(3), pair(5), pair(7)),
        4 => (single(1), single(2), single(3), 255),
        5 => (single(1), single(2), single(3), single(4)),
        _ => (255, 0, 0, 255),
    };
    if alpha == 255 { format!("#{red:02X}{green:02X}{blue:02X}") } else { format!("#{red:02X}{green:02X}{blue:02X}{alpha:02X}") }
}

/// A selector or classifier string `(type|*)(.modifier)*(:language)?` as `parseClassifierString` splits it.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenSelector {
    /// Empty for an invalid selector (`$invalid`), which matches nothing.
    pub token_type: String,
    /// In reverse order of the string, as upstream collects them.
    pub modifiers: Vec<String>,
    pub language: Option<String>,
}

/// `parseClassifierString`, also `parseTokenSelector`; `default_language` applies when the string names none.
pub fn parse_classifier_string(classifier: &str, default_language: Option<&str>) -> TokenSelector {
    let mut end = classifier.len();
    let mut language = default_language.map(str::to_owned);
    let mut modifiers = Vec::new();
    for (index, separator) in classifier.char_indices().rev() {
        if separator == ':' || separator == '.' {
            let segment = classifier[index + 1..end].to_owned();
            end = index;
            if separator == ':' {
                language = Some(segment);
            } else {
                modifiers.push(segment);
            }
        }
    }
    TokenSelector { token_type: classifier[..end].to_owned(), modifiers, language }
}

impl TokenSelector {
    /// `TokenSelector.match`: -1 when the selector does not apply, else 10 for a language, 100 minus the super type
    /// level for a type (none for `*`), and 100 per modifier.
    pub fn score(&self, registry: &TokenClassificationRegistry, token_type: &str, modifiers: &[&str], language: &str) -> i32 {
        if self.token_type.is_empty() {
            return -1;
        }
        let mut score = 0;
        if let Some(selector_language) = &self.language {
            if selector_language != language {
                return -1;
            }
            score += 10;
        }
        if self.token_type != "*" {
            let Some(level) = registry.type_hierarchy(token_type).iter().position(|id| *id == self.token_type) else { return -1 };
            score += 100 - level as i32;
        }
        if !self.modifiers.iter().all(|modifier| modifiers.contains(&modifier.as_str())) {
            return -1;
        }
        score + self.modifiers.len() as i32 * 100
    }
}

/// `TokenStyleValue`: a style literal or a classifier string resolved in the theme.
#[derive(Clone, Debug)]
pub enum TokenStyleValue {
    Style(TokenStyle),
    Classifier(String),
}

/// `TokenStyleDefaults`: the TextMate scopes to probe (`ProbeScope`s, outermost scope first), else the value of the
/// theme kind.
#[derive(Clone, Debug, Default)]
pub struct TokenStyleDefaults {
    pub scopes_to_probe: Option<Vec<Vec<String>>>,
    pub light: Option<TokenStyleValue>,
    pub dark: Option<TokenStyleValue>,
    pub hc_dark: Option<TokenStyleValue>,
    pub hc_light: Option<TokenStyleValue>,
}

/// `SemanticTokenDefaultRule`
#[derive(Clone, Debug)]
pub struct SemanticTokenDefaultRule {
    pub selector: TokenSelector,
    pub defaults: TokenStyleDefaults,
}

/// `SemanticTokenRule`: a `semanticTokenColors` entry of a theme or of the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticTokenRule {
    pub selector: TokenSelector,
    pub style: TokenStyle,
}

/// `TokenClassificationRegistry`: token types with their super types, modifiers, and the default styling rules.
#[derive(Clone, Debug)]
pub struct TokenClassificationRegistry {
    /// `tokenTypeById`: the super type of each type.
    token_types: HashMap<String, Option<String>>,
    token_modifiers: Vec<String>,
    default_rules: Vec<SemanticTokenDefaultRule>,
}

impl TokenClassificationRegistry {
    /// `createDefaultTokenClassificationRegistry`: the built-in types, modifiers and default rules.
    pub fn new() -> Self {
        let mut registry = TokenClassificationRegistry { token_types: HashMap::new(), token_modifiers: Vec::new(), default_rules: Vec::new() };
        let types: [(&str, &[&str], Option<&str>); 24] = [
            ("comment", &["comment"], None),
            ("string", &["string"], None),
            ("keyword", &["keyword.control"], None),
            ("number", &["constant.numeric"], None),
            ("regexp", &["constant.regexp"], None),
            ("operator", &["keyword.operator"], None),
            ("namespace", &["entity.name.namespace"], None),
            ("type", &["entity.name.type", "support.type"], None),
            ("struct", &["entity.name.type.struct"], None),
            ("class", &["entity.name.type.class", "support.class"], None),
            ("interface", &["entity.name.type.interface"], None),
            ("enum", &["entity.name.type.enum"], None),
            ("typeParameter", &["entity.name.type.parameter"], None),
            ("function", &["entity.name.function", "support.function"], None),
            ("member", &[], Some("method")),
            ("method", &["entity.name.function.member", "support.function"], None),
            ("macro", &["entity.name.function.preprocessor"], None),
            ("variable", &["variable.other.readwrite", "entity.name.variable"], None),
            ("parameter", &["variable.parameter"], None),
            ("property", &["variable.other.property"], None),
            ("enumMember", &["variable.other.enummember"], None),
            ("event", &["variable.other.event"], None),
            ("decorator", &["entity.name.decorator", "entity.name.function"], None),
            ("label", &[], None),
        ];
        for (id, scopes, super_type) in types {
            registry.register_token_type(id, super_type);
            registry.register_default(id, scopes);
        }
        for modifier in ["declaration", "documentation", "static", "abstract", "deprecated", "modification", "async", "readonly"] {
            registry.register_token_modifier(modifier);
        }
        let modifier_defaults: [(&str, &[&str]); 11] = [
            ("variable.readonly", &["variable.other.constant"]),
            ("property.readonly", &["variable.other.constant.property"]),
            ("type.defaultLibrary", &["support.type"]),
            ("class.defaultLibrary", &["support.class"]),
            ("interface.defaultLibrary", &["support.class"]),
            ("variable.defaultLibrary", &["support.variable", "support.other.variable"]),
            ("variable.defaultLibrary.readonly", &["support.constant"]),
            ("property.defaultLibrary", &["support.variable.property"]),
            ("property.defaultLibrary.readonly", &["support.constant.property"]),
            ("function.defaultLibrary", &["support.function"]),
            ("member.defaultLibrary", &["support.function"]),
        ];
        for (selector, scopes) in modifier_defaults {
            registry.register_default(selector, scopes);
        }
        registry
    }

    /// The `registerTokenStyleDefault` helper of `createDefaultTokenClassificationRegistry`: each scope a one-scope probe.
    fn register_default(&mut self, selector: &str, scopes: &[&str]) {
        let scopes_to_probe = scopes.iter().map(|scope| vec![scope.to_string()]).collect();
        self.register_token_style_default(parse_classifier_string(selector, None), TokenStyleDefaults { scopes_to_probe: Some(scopes_to_probe), ..Default::default() });
    }

    /// `registerTokenType`; a type registered again replaces the earlier one.
    pub fn register_token_type(&mut self, id: &str, super_type: Option<&str>) {
        self.token_types.insert(id.to_owned(), super_type.map(str::to_owned));
    }

    /// `registerTokenModifier`
    pub fn register_token_modifier(&mut self, id: &str) {
        self.token_modifiers.push(id.to_owned());
    }

    /// `registerTokenStyleDefault`
    pub fn register_token_style_default(&mut self, selector: TokenSelector, defaults: TokenStyleDefaults) {
        self.default_rules.push(SemanticTokenDefaultRule { selector, defaults });
    }

    /// `getTokenModifiers`, in registration order (modifier `n` is bit `1 << n`).
    pub fn token_modifiers(&self) -> &[String] {
        &self.token_modifiers
    }

    /// `getTokenStylingDefaultRules`
    pub fn default_rules(&self) -> &[SemanticTokenDefaultRule] {
        &self.default_rules
    }

    /// `getTypeHierarchy`: the type, then its super types.
    fn type_hierarchy<'a>(&'a self, token_type: &'a str) -> Vec<&'a str> {
        let mut hierarchy = vec![token_type];
        while let Some(Some(super_type)) = self.token_types.get(*hierarchy.last().unwrap()) {
            // Upstream loops forever on a super type cycle.
            if hierarchy.contains(&super_type.as_str()) {
                break;
            }
            hierarchy.push(super_type);
        }
        hierarchy
    }
}

/// `readSemanticTokenRule`: a color string, or an object with `foreground`, `fontStyle`, `bold`, `italic`,
/// `underline`, `strikethrough`; anything else is no rule.
pub fn read_semantic_token_rule(selector: &str, settings: &Value) -> Option<SemanticTokenRule> {
    let style = match settings {
        Value::String(color) => TokenStyle::from_settings(Some(color), None, None, None, None, None),
        Value::Object(object) => {
            let is_setting = ["foreground", "fontStyle"].iter().any(|key| object.get(*key).is_some_and(Value::is_string))
                || ["italic", "underline", "strikethrough", "bold"].iter().any(|key| object.get(*key).is_some_and(Value::is_boolean));
            if !is_setting {
                return None;
            }
            // A present non-string `fontStyle` still unsets every flag, as upstream's regex runs on its string form.
            let font_style = object.get("fontStyle").map(|font_style| font_style.as_str().unwrap_or(""));
            let flag = |key: &str| object.get(key).and_then(Value::as_bool);
            TokenStyle::from_settings(object.get("foreground").and_then(Value::as_str), font_style, flag("bold"), flag("underline"), flag("strikethrough"), flag("italic"))
        }
        _ => return None,
    };
    Some(SemanticTokenRule { selector: parse_classifier_string(selector, None), style })
}

/// `readSemanticTokenRules`: the rules of a `semanticTokenColors` object in order, `[theme]` keys skipped.
pub fn read_semantic_token_rules(semantic_token_colors: &Value, rules: &mut Vec<SemanticTokenRule>) {
    for (selector, settings) in semantic_token_colors.as_object().into_iter().flatten() {
        if selector.starts_with('[') && selector.ends_with(']') {
            continue;
        }
        rules.extend(read_semantic_token_rule(selector, settings));
    }
}

/// The semantic part of `_loadColorTheme` for one theme file; call it for each file of the `include` chain, the
/// included file first. Returns the file's rules and ORs its `semanticHighlighting` flag into `semantic_highlighting`.
pub fn read_theme_file_semantics(theme_file: &Value, rules: &mut Vec<SemanticTokenRule>, semantic_highlighting: &mut bool) {
    // A JSON theme in the tmTheme `settings` format has neither.
    if theme_file["settings"].is_array() {
        return;
    }
    *semantic_highlighting = *semantic_highlighting || is_truthy(&theme_file["semanticHighlighting"]);
    read_semantic_token_rules(&theme_file["semanticTokenColors"], rules);
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `ColorThemeData.setCustomSemanticTokenColors`: the `editor.semanticTokenColorCustomizations` value and its merged
/// `[theme]` block (`theme_specific`); appends the custom rules and returns the `enabled` override.
pub fn read_semantic_token_customizations(customizations: &Value, theme_specific: Option<&Value>, rules: &mut Vec<SemanticTokenRule>) -> Option<bool> {
    let mut enabled = customizations["enabled"].as_bool();
    read_semantic_token_rules(&customizations["rules"], rules);
    if let Some(theme_specific) = theme_specific.filter(|value| value.is_object()) {
        enabled = theme_specific["enabled"].as_bool().or(enabled);
        read_semantic_token_rules(&theme_specific["rules"], rules);
    }
    enabled
}

/// The `ColorThemeData.semanticHighlighting` getter: `editor.semanticTokenColorCustomizations.enabled`, else the
/// deprecated `editor.tokenColorCustomizations.semanticHighlighting`, else the theme file's flag.
pub fn semantic_highlighting(custom: Option<bool>, custom_deprecated: Option<bool>, theme_semantic_highlighting: bool) -> bool {
    custom.or(custom_deprecated).unwrap_or(theme_semantic_highlighting)
}

/// `isSemanticColoringEnabled`: `editor.semanticHighlighting.enabled` when it is a boolean, else
/// (`"configuredByTheme"`) the theme's `semanticHighlighting`.
pub fn is_semantic_coloring_enabled(setting: &Value, theme_semantic_highlighting: bool) -> bool {
    setting.as_bool().unwrap_or(theme_semantic_highlighting)
}

/// A TextMate theme rule as `resolveScopes` reads it, its selector compiled by `getScopeMatcher`.
struct ScopeRule {
    matchers: Vec<ScopeMatcher>,
    foreground: Option<String>,
    font_style: Option<String>,
}

impl ScopeRule {
    fn new(rule: &Value) -> ScopeRule {
        let settings = &rule["settings"];
        let mut matchers = Vec::new();
        if settings.is_object() {
            match &rule["scope"] {
                Value::String(scope) => create_matchers(scope, &mut matchers),
                Value::Array(scopes) => scopes.iter().filter_map(Value::as_str).for_each(|scope| create_matchers(scope, &mut matchers)),
                _ => {}
            }
        }
        let foreground = settings["foreground"].as_str().filter(|color| !color.is_empty()).map(str::to_owned);
        ScopeRule { matchers, foreground, font_style: settings["fontStyle"].as_str().map(str::to_owned) }
    }

    fn score(&self, scope: &[String]) -> i64 {
        self.matchers.iter().map(|matcher| matcher.score(scope)).max().unwrap_or(-1)
    }
}

/// The semantic styling of a color theme: `ColorThemeData.getTokenStyle` and its inputs.
pub struct SemanticTheme {
    registry: TokenClassificationRegistry,
    kind: ThemeKind,
    /// `semanticTokenRules` then `customSemanticTokenRules`.
    rules: Vec<SemanticTokenRule>,
    /// `themeTokenColors` then `customTokenColors`.
    scope_rules: Vec<ScopeRule>,
}

impl SemanticTheme {
    /// `rules` are the theme's semantic rules followed by the custom ones; `textmate_rules` the theme's `tokenColors`
    /// followed by the customizations' (e.g. `theme.tokens.rules()`).
    pub fn new(registry: TokenClassificationRegistry, kind: ThemeKind, rules: Vec<SemanticTokenRule>, textmate_rules: &[Value]) -> Self {
        SemanticTheme { registry, kind, rules, scope_rules: textmate_rules.iter().map(ScopeRule::new).collect() }
    }

    /// `ColorThemeData.getTokenStyleMetadata` with the color as a string where upstream returns its color index.
    pub fn get_token_style_metadata(&self, type_with_language: &str, modifiers: &[&str], default_language: &str) -> TokenStyle {
        let classifier = parse_classifier_string(type_with_language, Some(default_language));
        self.get_token_style(&classifier.token_type, modifiers, classifier.language.as_deref().unwrap_or(default_language), true)
    }

    /// `ColorThemeData.getTokenStyle`: per property the best scoring semantic rule (a later rule wins a tie), the
    /// properties no rule sets from the matching default rules: their probed TextMate scopes, else their value for
    /// the theme kind.
    pub fn get_token_style(&self, token_type: &str, modifiers: &[&str], language: &str, use_default: bool) -> TokenStyle {
        let mut result = TokenStyle::default();
        let mut scores = [-1; 5];
        for rule in &self.rules {
            let score = rule.selector.score(&self.registry, token_type, modifiers, language);
            if score >= 0 {
                process_style(&mut result, &mut scores, score, &rule.style);
            }
        }
        for score in &mut scores {
            if *score != -1 {
                *score = i32::MAX;
            }
        }
        for rule in self.registry.default_rules() {
            let score = rule.selector.score(&self.registry, token_type, modifiers, language);
            if score < 0 {
                continue;
            }
            let mut style = rule.defaults.scopes_to_probe.as_ref().and_then(|scopes| self.resolve_scopes(scopes));
            if style.is_none() && use_default {
                style = self.default_for_kind(&rule.defaults).map(|value| self.resolve_token_style_value(value));
            }
            if let Some(style) = style {
                process_style(&mut result, &mut scores, score, &style);
            }
        }
        result
    }

    /// `rule.defaults[this.type]`
    fn default_for_kind<'a>(&self, defaults: &'a TokenStyleDefaults) -> Option<&'a TokenStyleValue> {
        match self.kind {
            ThemeKind::Light => defaults.light.as_ref(),
            ThemeKind::Dark => defaults.dark.as_ref(),
            ThemeKind::HighContrastDark => defaults.hc_dark.as_ref(),
            ThemeKind::HighContrastLight => defaults.hc_light.as_ref(),
        }
    }

    /// `ColorThemeData.resolveTokenStyleValue`
    pub fn resolve_token_style_value(&self, value: &TokenStyleValue) -> TokenStyle {
        match value {
            TokenStyleValue::Style(style) => style.clone(),
            TokenStyleValue::Classifier(classifier) => {
                let classifier = parse_classifier_string(classifier, Some(""));
                let modifiers: Vec<&str> = classifier.modifiers.iter().map(String::as_str).collect();
                self.get_token_style(&classifier.token_type, &modifiers, classifier.language.as_deref().unwrap_or(""), true)
            }
        }
    }

    /// `ColorThemeData.resolveScopes`: the first probe scope some TextMate rule matches, styled by the best scoring
    /// rule with a foreground and the best with a `fontStyle` (a later rule wins a tie).
    pub fn resolve_scopes(&self, scopes: &[Vec<String>]) -> Option<TokenStyle> {
        for scope in scopes {
            let (mut foreground, mut foreground_score) = (None, -1);
            let (mut font_style, mut font_style_score) = (None, -1);
            for rule in &self.scope_rules {
                let score = rule.score(scope);
                if score < 0 {
                    continue;
                }
                if score >= foreground_score && rule.foreground.is_some() {
                    (foreground, foreground_score) = (rule.foreground.as_deref(), score);
                }
                if score >= font_style_score && rule.font_style.is_some() {
                    (font_style, font_style_score) = (rule.font_style.as_deref(), score);
                }
            }
            if foreground.is_some() || font_style.is_some() {
                return Some(TokenStyle::from_settings(foreground, font_style, None, None, None, None));
            }
        }
        None
    }
}

/// `_processStyle` of `getTokenStyle`: each property the style sets replaces the result's when it scores at least as high.
fn process_style(result: &mut TokenStyle, scores: &mut [i32; 5], score: i32, style: &TokenStyle) {
    if style.foreground.is_some() && scores[0] <= score {
        scores[0] = score;
        result.foreground = style.foreground.clone();
    }
    let flags = [(1, style.bold, &mut result.bold), (2, style.underline, &mut result.underline), (3, style.strikethrough, &mut result.strikethrough), (4, style.italic, &mut result.italic)];
    for (index, value, target) in flags {
        if value.is_some() && scores[index] <= score {
            scores[index] = score;
            *target = value;
        }
    }
}

#[cfg(test)]
mod tests;
