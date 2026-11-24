//! Token colors as vscode-textmate's theme.ts resolves them: the theme's `tokenColors` rules in a
//! trie by scope segment, the most specific rule (longest scope, then parent scopes) winning.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use crate::grammar::ScopeStack;

pub const FONT_STYLE_NOT_SET: i32 = -1;
pub const ITALIC: i32 = 1;
pub const BOLD: i32 = 2;
pub const UNDERLINE: i32 = 4;
pub const STRIKETHROUGH: i32 = 8;

/// The style of a token: color ids index `Theme::color_map`, 0 means "not set".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub font_style: i32,
    pub foreground: usize,
    pub background: usize,
}

pub struct Theme {
    /// Index 0 is unused, as in vscode-textmate's `ColorMap`.
    color_map: Vec<String>,
    defaults: Style,
    root: TrieElement,
    match_cache: RefCell<HashMap<String, Rc<Vec<TrieRule>>>>,
}

struct ParsedRule {
    scope: String,
    parent_scopes: Option<Vec<String>>,
    index: usize,
    font_style: i32,
    foreground: Option<String>,
    background: Option<String>,
}

impl Theme {
    /// `rules` are `IRawThemeSetting`s: `{ scope?, settings: { foreground?, background?, fontStyle? } }`.
    pub fn new(rules: &[Value]) -> Theme {
        let mut parsed = parse_theme(rules);
        parsed.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| str_arr_cmp(&a.parent_scopes, &b.parent_scopes)).then(a.index.cmp(&b.index)));
        let (mut font_style, mut foreground, mut background) = (0, "#000000".to_string(), "#ffffff".to_string());
        let mut rest = parsed.as_slice();
        while let Some((rule, tail)) = rest.split_first().filter(|(rule, _)| rule.scope.is_empty()) {
            if rule.font_style != FONT_STYLE_NOT_SET {
                font_style = rule.font_style;
            }
            if let Some(color) = &rule.foreground {
                foreground = color.clone();
            }
            if let Some(color) = &rule.background {
                background = color.clone();
            }
            rest = tail;
        }
        let mut color_map = vec![String::new()];
        let defaults = Style { font_style, foreground: color_id(&mut color_map, Some(&foreground)), background: color_id(&mut color_map, Some(&background)) };
        let mut root = TrieElement { main_rule: TrieRule { scope_depth: 0, parent_scopes: Vec::new(), font_style: FONT_STYLE_NOT_SET, foreground: 0, background: 0 }, rules_with_parent_scopes: Vec::new(), children: HashMap::new() };
        for rule in rest {
            let style = Style { font_style: rule.font_style, foreground: color_id(&mut color_map, rule.foreground.as_deref()), background: color_id(&mut color_map, rule.background.as_deref()) };
            root.insert(0, &rule.scope, rule.parent_scopes.as_deref(), style);
        }
        Theme { color_map, defaults, root, match_cache: RefCell::default() }
    }

    pub fn color_map(&self) -> &[String] {
        &self.color_map
    }

    pub fn defaults(&self) -> Style {
        self.defaults
    }

    /// `Theme.match`: the style the rules give the innermost scope of `path`, if any rule applies.
    pub fn match_scope(&self, path: &ScopeStack) -> Option<Style> {
        let rules = self.match_cache.borrow_mut().entry(path.scope_name.clone()).or_insert_with(|| Rc::new(self.root.find(&path.scope_name))).clone();
        let rule = rules.iter().find(|rule| scope_path_matches_parent_scopes(path.parent.as_deref(), &rule.parent_scopes))?;
        Some(Style { font_style: rule.font_style, foreground: rule.foreground, background: rule.background })
    }

    /// The style a token with these scopes gets: each scope's match merged over its parent's, as
    /// `AttributedScopeStack.pushAttributed` merges token attributes.
    pub fn style(&self, path: &ScopeStack) -> Style {
        let mut style = match &path.parent {
            Some(parent) => self.style(parent),
            None => self.defaults,
        };
        if let Some(matched) = self.match_scope(path) {
            if matched.font_style != FONT_STYLE_NOT_SET {
                style.font_style = matched.font_style;
            }
            if matched.foreground != 0 {
                style.foreground = matched.foreground;
            }
            if matched.background != 0 {
                style.background = matched.background;
            }
        }
        style
    }
}

fn color_id(color_map: &mut Vec<String>, color: Option<&str>) -> usize {
    let Some(color) = color else { return 0 };
    let color = color.to_uppercase();
    if let Some(id) = color_map.iter().position(|c| *c == color) {
        return id;
    }
    color_map.push(color);
    color_map.len() - 1
}

fn parse_theme(rules: &[Value]) -> Vec<ParsedRule> {
    let mut result = Vec::new();
    for (index, entry) in rules.iter().enumerate() {
        let settings = &entry["settings"];
        if !settings.is_object() {
            continue;
        }
        let scopes: Vec<String> = match &entry["scope"] {
            Value::String(scope) => scope.trim_start_matches(',').trim_end_matches(',').split(',').map(str::to_string).collect(),
            Value::Array(scopes) => scopes.iter().map(|scope| scope.as_str().unwrap_or_default().to_string()).collect(),
            _ => vec![String::new()],
        };
        let mut font_style = FONT_STYLE_NOT_SET;
        if let Some(text) = settings["fontStyle"].as_str() {
            font_style = 0;
            for segment in text.split(' ') {
                font_style |= match segment {
                    "italic" => ITALIC,
                    "bold" => BOLD,
                    "underline" => UNDERLINE,
                    "strikethrough" => STRIKETHROUGH,
                    _ => 0,
                };
            }
        }
        let color = |key: &str| settings[key].as_str().filter(|hex| is_valid_hex_color(hex)).map(str::to_string);
        for scope in scopes {
            let segments: Vec<&str> = scope.trim().split(' ').collect();
            let (last, parents) = segments.split_last().unwrap();
            let parent_scopes = (!parents.is_empty()).then(|| parents.iter().rev().map(|s| s.to_string()).collect());
            result.push(ParsedRule { scope: last.to_string(), parent_scopes, index, font_style, foreground: color("foreground"), background: color("background") });
        }
    }
    result
}

fn is_valid_hex_color(hex: &str) -> bool {
    hex.strip_prefix('#').is_some_and(|digits| matches!(digits.len(), 3 | 4 | 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `strArrCmp`: null first, then shorter first, then element by element.
fn str_arr_cmp(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) if a.len() == b.len() => a.cmp(b),
        (Some(a), Some(b)) => a.len().cmp(&b.len()),
    }
}

fn scope_path_matches_parent_scopes(mut path: Option<&ScopeStack>, parent_scopes: &[String]) -> bool {
    let mut index = 0;
    while index < parent_scopes.len() {
        let mut pattern = parent_scopes[index].as_str();
        let mut must_match = false;
        if pattern == ">" {
            if index == parent_scopes.len() - 1 {
                return false;
            }
            index += 1;
            pattern = &parent_scopes[index];
            must_match = true;
        }
        loop {
            let Some(current) = path else { return false };
            if matches_scope(&current.scope_name, pattern) {
                break;
            }
            if must_match {
                return false;
            }
            path = current.parent.as_deref();
        }
        path = path.unwrap().parent.as_deref();
        index += 1;
    }
    true
}

fn matches_scope(scope_name: &str, pattern: &str) -> bool {
    scope_name == pattern || (scope_name.starts_with(pattern) && scope_name.as_bytes().get(pattern.len()) == Some(&b'.'))
}

#[derive(Clone)]
struct TrieRule {
    scope_depth: usize,
    parent_scopes: Vec<String>,
    font_style: i32,
    foreground: usize,
    background: usize,
}

impl TrieRule {
    fn accept_overwrite(&mut self, scope_depth: usize, style: Style) {
        self.scope_depth = self.scope_depth.max(scope_depth);
        if style.font_style != FONT_STYLE_NOT_SET {
            self.font_style = style.font_style;
        }
        if style.foreground != 0 {
            self.foreground = style.foreground;
        }
        if style.background != 0 {
            self.background = style.background;
        }
    }
}

struct TrieElement {
    main_rule: TrieRule,
    rules_with_parent_scopes: Vec<TrieRule>,
    children: HashMap<String, TrieElement>,
}

fn split_head(scope: &str) -> (&str, &str) {
    scope.split_once('.').unwrap_or((scope, ""))
}

impl TrieElement {
    fn find(&self, scope: &str) -> Vec<TrieRule> {
        if !scope.is_empty() {
            let (head, tail) = split_head(scope);
            if let Some(child) = self.children.get(head) {
                return child.find(tail);
            }
        }
        let mut rules = self.rules_with_parent_scopes.clone();
        rules.push(self.main_rule.clone());
        rules.sort_by(cmp_by_specificity);
        rules
    }

    fn insert(&mut self, scope_depth: usize, scope: &str, parent_scopes: Option<&[String]>, style: Style) {
        if scope.is_empty() {
            self.insert_here(scope_depth, parent_scopes, style);
            return;
        }
        let (head, tail) = split_head(scope);
        let child = self.children.entry(head.to_string()).or_insert_with(|| TrieElement {
            main_rule: self.main_rule.clone(),
            rules_with_parent_scopes: self.rules_with_parent_scopes.clone(),
            children: HashMap::new(),
        });
        child.insert(scope_depth + 1, tail, parent_scopes, style);
    }

    fn insert_here(&mut self, scope_depth: usize, parent_scopes: Option<&[String]>, mut style: Style) {
        let Some(parent_scopes) = parent_scopes else {
            self.main_rule.accept_overwrite(scope_depth, style);
            return;
        };
        if let Some(rule) = self.rules_with_parent_scopes.iter_mut().find(|rule| rule.parent_scopes == parent_scopes) {
            rule.accept_overwrite(scope_depth, style);
            return;
        }
        // A new rule with parent scopes inherits what it leaves unset from the main rule.
        if style.font_style == FONT_STYLE_NOT_SET {
            style.font_style = self.main_rule.font_style;
        }
        if style.foreground == 0 {
            style.foreground = self.main_rule.foreground;
        }
        if style.background == 0 {
            style.background = self.main_rule.background;
        }
        self.rules_with_parent_scopes.push(TrieRule {
            scope_depth,
            parent_scopes: parent_scopes.to_vec(),
            font_style: style.font_style,
            foreground: style.foreground,
            background: style.background,
        });
    }
}

/// `_cmpBySpecificity`: deeper scope first, then longer parent scopes (skipping `>`), then more of them.
fn cmp_by_specificity(a: &TrieRule, b: &TrieRule) -> std::cmp::Ordering {
    if a.scope_depth != b.scope_depth {
        return b.scope_depth.cmp(&a.scope_depth);
    }
    let (mut ai, mut bi) = (0, 0);
    loop {
        if a.parent_scopes.get(ai).is_some_and(|s| s == ">") {
            ai += 1;
        }
        if b.parent_scopes.get(bi).is_some_and(|s| s == ">") {
            bi += 1;
        }
        if ai >= a.parent_scopes.len() || bi >= b.parent_scopes.len() {
            break;
        }
        let diff = b.parent_scopes[bi].len().cmp(&a.parent_scopes[ai].len());
        if diff != std::cmp::Ordering::Equal {
            return diff;
        }
        ai += 1;
        bi += 1;
    }
    b.parent_scopes.len().cmp(&a.parent_scopes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn most_specific_rule_wins_and_children_inherit() {
        let theme = Theme::new(&[
            json!({ "settings": { "foreground": "#F8F8F2", "background": "#272822" } }),
            json!({ "scope": "string", "settings": { "foreground": "#E6DB74" } }),
            json!({ "scope": "meta.tag string", "settings": { "foreground": "#aaaaaa", "fontStyle": "italic" } }),
            json!({ "scope": ["string.quoted"], "settings": { "fontStyle": "bold" } }),
        ]);
        let color = |path: &ScopeStack| theme.color_map()[theme.style(path).foreground].clone();
        let plain = ScopeStack::from_names(&["source.js", "string.quoted.double"]);
        assert_eq!(color(&plain), "#E6DB74");
        assert_eq!(theme.style(&plain).font_style, BOLD);
        let in_tag = ScopeStack::from_names(&["source.js", "meta.tag", "string.unquoted"]);
        assert_eq!(color(&in_tag), "#AAAAAA");
        // The deeper `string.quoted` node wins over the parent-scoped rule, as in vscode-textmate.
        assert_eq!(color(&ScopeStack::from_names(&["source.js", "meta.tag", "string.quoted.double"])), "#E6DB74");
        assert_eq!(color(&ScopeStack::from_names(&["source.js"])), "#F8F8F2");
    }
}
