use std::path::Path;

use serde_json::{Value, json};

use super::provider_styling::{InvalidSemanticTokensEdit, SemanticTokensEdit, StyledRange};
use super::*;

const UNDEFINED_STYLE: [Option<bool>; 3] = [None, None, None];
const UNSET_STYLE: [Option<bool>; 3] = [Some(false), Some(false), Some(false)];

/// What tokenStyleResolving.test.ts `tokenStyleAsString` compares: the foreground, then bold, underline, italic.
fn summary(style: &TokenStyle) -> (Option<String>, [Option<bool>; 3]) {
    (style.foreground.clone(), [style.bold, style.underline, style.italic])
}

/// `ts(foreground, styleFlags)`, flags as bold, underline, italic.
fn ts(foreground: &str, flags: [Option<bool>; 3]) -> (Option<String>, [Option<bool>; 3]) {
    (Some(color_from_hex(foreground)), flags)
}

/// `assertTokenStyles`: each key is `type.modifier...`.
fn assert_token_styles(theme: &SemanticTheme, expected: &[(&str, (Option<String>, [Option<bool>; 3]))], language: &str) {
    for (classifier, expected) in expected {
        let mut parts = classifier.split('.');
        let token_type = parts.next().unwrap();
        let modifiers: Vec<&str> = parts.collect();
        assert_eq!(summary(&theme.get_token_style_metadata(token_type, &modifiers, language)), *expected, "{classifier}");
    }
}

fn custom_theme(registry: TokenClassificationRegistry, semantic_rules: Value, textmate_rules: Value) -> SemanticTheme {
    let mut rules = Vec::new();
    read_semantic_token_customizations(&json!({ "enabled": true, "rules": semantic_rules }), None, &mut rules);
    SemanticTheme::new(registry, ThemeKind::Dark, rules, textmate_rules.as_array().unwrap())
}

fn app_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&crate::theme::strip_jsonc(&std::fs::read_to_string(path).unwrap())).unwrap()
}

#[test]
fn color_defaults() {
    let theme_file = read_json(&app_root().join("src/vs/workbench/services/themes/test/node/color-theme.json"));
    let theme = SemanticTheme::new(TokenClassificationRegistry::new(), ThemeKind::Dark, Vec::new(), theme_file["tokenColors"].as_array().unwrap());
    assert_token_styles(
        &theme,
        &[
            ("comment", ts("#000000", UNDEFINED_STYLE)),
            ("variable", ts("#111111", UNSET_STYLE)),
            ("type", ts("#333333", [Some(false), Some(true), Some(false)])),
            ("function", ts("#333333", UNSET_STYLE)),
            ("string", ts("#444444", UNDEFINED_STYLE)),
            ("number", ts("#555555", UNDEFINED_STYLE)),
            ("keyword", ts("#666666", UNDEFINED_STYLE)),
        ],
        "typescript",
    );
}

fn probe(scopes: &[&[&str]]) -> Vec<Vec<String>> {
    scopes.iter().map(|scope| scope.iter().map(|s| s.to_string()).collect()).collect()
}

#[test]
fn resolve_scopes() {
    let textmate_rules = json!([
        { "scope": "variable", "settings": { "fontStyle": "", "foreground": "#F8F8F2" } },
        { "scope": "keyword.operator", "settings": { "fontStyle": "italic bold underline", "foreground": "#F92672" } },
        { "scope": "storage", "settings": { "fontStyle": "italic", "foreground": "#F92672" } },
        { "scope": ["storage.type", "meta.structure.dictionary.json string.quoted.double.json"], "settings": { "foreground": "#66D9EF" } },
        { "scope": "entity.name.type, entity.name.class, entity.name.namespace, entity.name.scope-resolution", "settings": { "fontStyle": "underline", "foreground": "#A6E22E" } },
    ]);
    let theme = custom_theme(TokenClassificationRegistry::new(), json!({}), textmate_rules);
    let resolve = |scopes: &[&[&str]]| theme.resolve_scopes(&probe(scopes)).map(|style| summary(&style));

    assert_eq!(resolve(&[&["variable"]]), Some(ts("#F8F8F2", UNSET_STYLE)), "variable");
    assert_eq!(resolve(&[&["keyword.operator"]]), Some(ts("#F92672", [Some(true), Some(true), Some(true)])), "keyword");
    assert_eq!(resolve(&[&["keyword"]]), None, "keyword");
    assert_eq!(resolve(&[&["keyword.operators"]]), None, "keyword.operators");
    assert_eq!(resolve(&[&["storage"]]), Some(ts("#F92672", [Some(false), Some(false), Some(true)])), "storage");
    assert_eq!(resolve(&[&["storage.type"]]), Some(ts("#66D9EF", [Some(false), Some(false), Some(true)])), "storage.type");
    assert_eq!(resolve(&[&["entity.name.class"]]), Some(ts("#A6E22E", [Some(false), Some(true), Some(false)])), "entity.name.class");
    assert_eq!(resolve(&[&["meta.structure.dictionary.json", "string.quoted.double.json"]]), Some(ts("#66D9EF", UNDEFINED_STYLE)), "json property");
    assert_eq!(resolve(&[&["source.json", "meta.structure.dictionary.json", "string.quoted.double.json"]]), Some(ts("#66D9EF", UNDEFINED_STYLE)), "json property");
    assert_eq!(resolve(&[&["keyword"], &["storage.type"], &["entity.name.class"]]), Some(ts("#66D9EF", [Some(false), Some(false), Some(true)])), "storage.type");
}

#[test]
fn resolve_scopes_match_most_specific() {
    let textmate_rules = json!([
        { "scope": "entity.name.type", "settings": { "fontStyle": "underline", "foreground": "#A6E22E" } },
        { "scope": "entity.name.type.class", "settings": { "foreground": "#FF00FF" } },
        { "scope": "entity.name", "settings": { "foreground": "#FFFFFF" } },
    ]);
    let theme = custom_theme(TokenClassificationRegistry::new(), json!({}), textmate_rules);
    let style = theme.resolve_scopes(&probe(&[&["entity.name.type.class"]])).unwrap();
    assert_eq!(summary(&style), ts("#FF00FF", [Some(false), Some(true), Some(false)]));
}

#[test]
fn rule_matching() {
    let rules = json!({
        "type": "#ff0000",
        "class": { "foreground": "#0000ff", "italic": true },
        "*.static": { "bold": true },
        "*.declaration": { "italic": true },
        "*.async.static": { "italic": true, "underline": true },
        "*.async": { "foreground": "#000fff", "underline": true },
    });
    let theme = custom_theme(TokenClassificationRegistry::new(), rules, json!([]));
    assert_token_styles(
        &theme,
        &[
            ("type", ts("#ff0000", UNDEFINED_STYLE)),
            ("type.static", ts("#ff0000", [Some(true), None, None])),
            ("type.static.declaration", ts("#ff0000", [Some(true), None, Some(true)])),
            ("class", ts("#0000ff", [None, None, Some(true)])),
            ("class.static.declaration", ts("#0000ff", [Some(true), None, Some(true)])),
            ("class.declaration", ts("#0000ff", [None, None, Some(true)])),
            ("class.declaration.async", ts("#000fff", [None, Some(true), Some(true)])),
            ("class.declaration.async.static", ts("#000fff", [Some(true), Some(true), Some(true)])),
        ],
        "typescript",
    );
}

#[test]
fn super_type() {
    let mut registry = TokenClassificationRegistry::new();
    registry.register_token_type("myTestInterface", Some("interface"));
    registry.register_token_type("myTestSubInterface", Some("myTestInterface"));

    let rules = json!({ "interface": "#ff0000", "myTestInterface": { "italic": true }, "interface.static": { "bold": true } });
    let theme = custom_theme(registry.clone(), rules, json!([]));
    assert_token_styles(&theme, &[("myTestSubInterface", ts("#ff0000", [None, None, Some(true)]))], "typescript");
    assert_token_styles(&theme, &[("myTestSubInterface.static", ts("#ff0000", [Some(true), None, Some(true)]))], "typescript");

    let rules = json!({ "interface": "#ff0000", "myTestInterface": { "foreground": "#ff00ff", "italic": true } });
    let theme = custom_theme(registry, rules, json!([]));
    assert_token_styles(&theme, &[("myTestSubInterface", ts("#ff00ff", [None, None, Some(true)]))], "typescript");
}

#[test]
fn language() {
    let rules = json!({
        "interface": "#fff000",
        "interface:java": "#ff0000",
        "interface.static": { "bold": true },
        "interface.static:typescript": { "italic": true },
    });
    let theme = custom_theme(TokenClassificationRegistry::new(), rules, json!([]));
    assert_token_styles(&theme, &[("interface", ts("#ff0000", UNDEFINED_STYLE))], "java");
    assert_token_styles(&theme, &[("interface", ts("#fff000", UNDEFINED_STYLE))], "typescript");
    assert_token_styles(&theme, &[("interface.static", ts("#ff0000", [Some(true), None, None]))], "java");
    assert_token_styles(&theme, &[("interface.static", ts("#fff000", [Some(true), None, Some(true)]))], "typescript");
}

#[test]
fn language_scope_resolving() {
    let mut registry = TokenClassificationRegistry::new();
    let probe_defaults = |scope: &str| TokenStyleDefaults { scopes_to_probe: Some(vec![vec![scope.to_owned()]]), ..Default::default() };
    registry.register_token_style_default(parse_classifier_string("type", Some("typescript1")), probe_defaults("entity.name.type.ts1"));
    registry.register_token_style_default(parse_classifier_string("type:javascript1", None), probe_defaults("entity.name.type.js1"));
    let textmate_rules = json!([
        { "scope": "entity.name.type", "settings": { "foreground": "#aa0000" } },
        { "scope": "entity.name.type.ts1", "settings": { "foreground": "#bb0000" } },
    ]);
    let theme = custom_theme(registry, json!({}), textmate_rules);
    assert_token_styles(&theme, &[("type", ts("#aa0000", UNDEFINED_STYLE))], "javascript1");
    assert_token_styles(&theme, &[("type", ts("#bb0000", UNDEFINED_STYLE))], "typescript1");
}

#[test]
fn theme_kind_default_resolves_a_classifier() {
    let mut registry = TokenClassificationRegistry::new();
    let defaults = TokenStyleDefaults { dark: Some(TokenStyleValue::Classifier("class".into())), light: Some(TokenStyleValue::Style(TokenStyle { bold: Some(true), ..Default::default() })), ..Default::default() };
    registry.register_token_type("widget", None);
    registry.register_token_style_default(parse_classifier_string("widget", None), defaults);
    let mut rules = Vec::new();
    read_semantic_token_rules(&json!({ "class": "#123" }), &mut rules);
    let dark = SemanticTheme::new(registry.clone(), ThemeKind::Dark, rules.clone(), &[]);
    assert_eq!(dark.get_token_style_metadata("widget", &[], "rust").foreground.as_deref(), Some("#112233"));
    let light = SemanticTheme::new(registry, ThemeKind::Light, rules, &[]);
    assert_eq!(summary(&light.get_token_style_metadata("widget", &[], "rust")), (None, [Some(true), None, None]));
}

/// dark_plus.json includes dark_vs.json: both files' `semanticTokenColors` and the TextMate rules of the loaded theme.
#[test]
fn dark_plus_theme() {
    let themes = exthost::scan::scan_extension(&app_root().join("extensions/theme-defaults"), true).unwrap();
    let loaded = crate::theme::load_theme(&[themes], "Dark+").unwrap();
    let directory = app_root().join("extensions/theme-defaults/themes");
    let (mut rules, mut semantic_highlighting) = (Vec::new(), false);
    for file in ["dark_vs.json", "dark_plus.json"] {
        read_theme_file_semantics(&read_json(&directory.join(file)), &mut rules, &mut semantic_highlighting);
    }
    assert!(semantic_highlighting);
    let theme = SemanticTheme::new(TokenClassificationRegistry::new(), loaded.kind, rules, loaded.tokens.rules());
    assert_eq!(theme.get_token_style_metadata("newOperator", &[], "cpp").foreground.as_deref(), Some("#C586C0"));
    assert_eq!(theme.get_token_style_metadata("function", &[], "typescript").foreground.as_deref(), Some("#DCDCAA"));
    assert_eq!(theme.get_token_style_metadata("unknownType", &[], "typescript"), TokenStyle::default());
}

#[test]
fn semantic_highlighting_settings() {
    let mut rules = Vec::new();
    let enabled = read_semantic_token_customizations(&json!({ "enabled": true, "rules": { "[Dark+]": { "x": 1 }, "type": "#fff" } }), Some(&json!({ "enabled": false, "rules": { "class": "#000" } })), &mut rules);
    assert_eq!(enabled, Some(false));
    assert_eq!(rules.iter().map(|rule| rule.selector.token_type.as_str()).collect::<Vec<_>>(), ["type", "class"]);
    assert!(semantic_highlighting(None, Some(true), false));
    assert!(!semantic_highlighting(Some(false), Some(true), true));
    assert!(semantic_highlighting(None, None, true));
    assert!(is_semantic_coloring_enabled(&json!("configuredByTheme"), true));
    assert!(!is_semantic_coloring_enabled(&json!(false), true));
    assert!(is_semantic_coloring_enabled(&json!(true), false));
}

#[test]
fn token_style_from_settings() {
    let style = TokenStyle::from_settings(Some("#abcd"), Some("bolditalic"), None, Some(true), None, None);
    assert_eq!(style, TokenStyle { foreground: Some("#AABBCCDD".into()), bold: Some(true), underline: Some(false), strikethrough: Some(false), italic: Some(true) });
    assert_eq!(color_from_hex("#ff00ffFF"), "#FF00FF");
    assert_eq!(color_from_hex("red"), "#FF0000");
    assert_eq!(read_semantic_token_rule("x", &json!({ "fontStyle": null, "bold": true })).unwrap().style.bold, Some(false));
    assert_eq!(read_semantic_token_rule("x", &json!({ "background": "#fff" })), None);
    let selector = parse_classifier_string("type.a.b:lang", None);
    assert_eq!((selector.token_type.as_str(), selector.modifiers.clone(), selector.language.as_deref()), ("type", vec!["b".to_owned(), "a".to_owned()], Some("lang")));
}

/// semanticTokensDto.test.ts `encodeSemanticTokensDto`, as the extension host side encodes the reply.
fn encode(dto: &SemanticTokensDto) -> Vec<u8> {
    let mut values = Vec::new();
    match dto {
        SemanticTokensDto::Full { id, data } => {
            values.extend([*id, 1, data.len() as u32]);
            values.extend(data);
        }
        SemanticTokensDto::Delta { id, deltas } => {
            values.extend([*id, 2, deltas.len() as u32]);
            for delta in deltas {
                let data = delta.data.as_deref().unwrap_or_default();
                values.extend([delta.start, delta.delete_count, data.len() as u32]);
                values.extend(data);
            }
        }
    }
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn assert_round_trip(dto: SemanticTokensDto) {
    assert_eq!(decode_semantic_tokens_dto(&encode(&dto)), Some(dto));
}

const PACKED: u32 = (1 << 24) + (2 << 16) + (3 << 8) + 4;

#[test]
fn dto_full_encoding() {
    assert_round_trip(SemanticTokensDto::Full { id: 12, data: vec![PACKED] });
}

#[test]
fn dto_delta_encoding() {
    assert_round_trip(SemanticTokensDto::Delta {
        id: 12,
        deltas: vec![
            SemanticTokensEdit { start: 0, delete_count: 4, data: None },
            SemanticTokensEdit { start: 15, delete_count: 0, data: Some(vec![PACKED]) },
            SemanticTokensEdit { start: 27, delete_count: 5, data: Some(vec![PACKED, 1, 2, 3, 4, 5, 6, 7, 8, 9]) },
        ],
    });
}

#[test]
fn dto_partial_array_buffer() {
    let shared = [PACKED, 1, 2, 3, 4, 5, PACKED];
    assert_round_trip(SemanticTokensDto::Delta {
        id: 12,
        deltas: vec![SemanticTokensEdit { start: 0, delete_count: 4, data: Some(shared[..1].to_vec()) }, SemanticTokensEdit { start: 15, delete_count: 0, data: Some(shared[1..].to_vec()) }],
    });
}

/// issue #94521: unusual backing array buffer.
#[test]
fn dto_misaligned_buffer() {
    let dto = SemanticTokensDto::Full { id: 5, data: vec![1, 2, 3, 4, 5] };
    let encoded = encode(&dto);
    for (prefix, suffix) in [(1, 1), (1, 4), (4, 1), (4, 4)] {
        let mut wrapped = vec![0u8; prefix];
        wrapped.extend(&encoded);
        wrapped.extend(vec![0u8; suffix]);
        assert_eq!(decode_semantic_tokens_dto(&wrapped[prefix..prefix + encoded.len()]), Some(dto.clone()), "{prefix} {suffix}");
    }
}

#[test]
fn dto_truncated_reply() {
    let encoded = encode(&SemanticTokensDto::Full { id: 5, data: vec![1, 2, 3] });
    assert_eq!(decode_semantic_tokens_dto(&encoded[..encoded.len() - 4]), None);
    assert_eq!(decode_semantic_tokens_dto(&encoded[..encoded.len() - 1]), None);
}

#[test]
fn edits_apply_back_to_front() {
    let previous = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    let edits = [SemanticTokensEdit { start: 2, delete_count: 3, data: Some(vec![20, 21]) }, SemanticTokensEdit { start: 8, delete_count: 0, data: Some(vec![80]) }];
    assert_eq!(apply_semantic_tokens_edits(&previous, &edits), Ok(vec![0, 1, 20, 21, 5, 6, 7, 80, 8, 9]));
    assert_eq!(apply_semantic_tokens_edits(&previous, &[]), Ok(previous.to_vec()));
}

/// documentSemanticTokens.test.ts 'issue #149412: VS Code hangs when bad semantic token data is received'.
#[test]
fn edit_start_past_the_previous_data_is_rejected() {
    let previous = [4294967293, 0, 7, 16, 0, 1, 4, 3, 11, 1];
    let edits = [SemanticTokensEdit { start: 4294967276, delete_count: 0, data: Some(vec![2, 0, 3, 11, 0]) }];
    assert_eq!(apply_semantic_tokens_edits(&previous, &edits), Err(InvalidSemanticTokensEdit::Start { edit_index: 0, start: 4294967276, previous_length: 10 }));
}

/// documentSemanticTokens.test.ts 'issue #322571: semantic token edits cannot delete more data than the previous result contains'.
#[test]
fn edit_deleting_more_than_the_previous_data_is_rejected() {
    let edits = [SemanticTokensEdit { start: 0, delete_count: 6, data: None }];
    assert_eq!(apply_semantic_tokens_edits(&[0, 0, 1, 0, 0], &edits), Err(InvalidSemanticTokensEdit::DeleteCount { previous_length: 5, delta_length: -6 }));
}

/// The semanticTokensProviderStyling.test.ts theme: type `stN` has foreground `N`, 0 being none.
fn numbered_style(token_type: &str, _modifiers: &[&str], _language: &str) -> TokenStyle {
    let number: u32 = token_type[2..].parse().unwrap();
    TokenStyle { foreground: (number != 0).then(|| number.to_string()), ..Default::default() }
}

fn legend(types: usize, modifiers: usize) -> SemanticTokensLegend {
    SemanticTokensLegend { token_types: (0..types).map(|i| format!("st{i}")).collect(), token_modifiers: (0..modifiers).map(|i| format!("stm{i}")).collect() }
}

/// `(line, start, end, foreground)` of every range.
fn flatten(lines: &[Vec<StyledRange>]) -> Vec<(usize, u32, u32, String)> {
    let mut result = Vec::new();
    for (line, ranges) in lines.iter().enumerate() {
        for range in ranges {
            result.push((line, range.start, range.end, range.style.foreground.clone().unwrap()));
        }
    }
    result
}

fn ranges(expected: &[(usize, u32, u32, u32)]) -> Vec<(usize, u32, u32, String)> {
    expected.iter().map(|&(line, start, end, foreground)| (line, start, end, foreground.to_string())).collect()
}

#[test]
fn issue_134973_invalid_semantic_tokens_should_be_handled_better() {
    let mut styling = SemanticTokensProviderStyling::new(legend(11, 0));
    #[rustfmt::skip]
    let data = [
        0, 13, 16, 1, 0,
        1, 2, 6, 2, 0,
        0, 7, 6, 3, 0,
        0, 15, 8, 4, 0,
        0, 17, 1, 5, 0,
        0, 7, 5, 6, 0,
        1, 12, 8, 7, 0,
        0, 19, 5, 8, 0,
        0, 7, 1, 9, 0,
        0, 4294967294, 5, 10, 0,
    ];
    let lines = to_multiline_tokens(&data, &mut styling, "java", 3, &numbered_style);
    let expected = [(0, 13, 29, 1), (1, 2, 8, 2), (1, 9, 15, 3), (1, 24, 32, 4), (1, 41, 42, 5), (1, 48, 53, 6), (2, 12, 20, 7), (2, 31, 36, 8), (2, 38, 39, 9)];
    assert_eq!(flatten(&lines), ranges(&expected));
}

#[test]
fn issue_148651_negative_token_values_do_not_hang() {
    let mut styling = SemanticTokensProviderStyling::new(legend(10, 3));
    #[rustfmt::skip]
    let data = [
        0, 0, 3, 0, 0,
        0, 4, 2, 2, 0,
        0, 2, 3, 8, 0,
        0, 3, 1, 9, 0,
        0, 1, 1, 10, 0,
        0, 1, 4, 8, 0,
        0, 4, 4294967292, 2, 0,
        0, 4294967292, 4294967294, 8, 0,
        0, 4294967294, 1, 9, 0,
        0, 1, 1, 10, 0,
        0, 1, 3, 8, 0,
        0, 3, 4294967291, 8, 0,
        0, 4294967291, 1, 9, 0,
        0, 1, 1, 10, 0,
        0, 1, 4, 8, 0,
    ];
    let lines = to_multiline_tokens(&data, &mut styling, "dockerfile", 1, &numbered_style);
    assert_eq!(flatten(&lines), ranges(&[(0, 4, 6, 2), (0, 6, 9, 8), (0, 9, 10, 9), (0, 11, 15, 8)]));
}

#[test]
fn issue_149130_overlapping_token_after_wrapped_delta() {
    let mut styling = SemanticTokensProviderStyling::new(legend(6, 3));
    let data = [0, 11, 1, 1, 0, 0, 4, 1, 1, 0, 0, 4294967289, 1, 1, 0];
    let lines = to_multiline_tokens(&data, &mut styling, "q", 1, &numbered_style);
    assert_eq!(flatten(&lines), ranges(&[(0, 11, 12, 1), (0, 15, 16, 1)]));
}

#[test]
fn get_metadata_passes_legend_modifiers() {
    let mut styling = SemanticTokensProviderStyling::new(SemanticTokensLegend { token_types: vec!["variable".into()], token_modifiers: vec!["static".into(), "readonly".into(), "async".into()] });
    let style_of = |_: &str, modifiers: &[&str], language: &str| TokenStyle { foreground: Some(format!("{}:{language}", modifiers.join(","))), ..Default::default() };
    assert_eq!(styling.get_metadata(0, 0b1101, "rust", &style_of).unwrap().foreground.as_deref(), Some("static,async:rust"));
    assert_eq!(styling.get_metadata(1, 0, "rust", &style_of), None);
}
