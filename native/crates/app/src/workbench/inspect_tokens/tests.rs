use std::path::Path;

use gpui::TestAppContext;
use serde_json::json;
use textmate::StandardTokenType;

use super::*;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;

#[test]
fn token_text_shows_tabs_as_arrows_spaces_as_dots_and_elides_past_40_characters() {
    assert_eq!(render_token_text("\ta b"), "\u{2192}a\u{00B7}b");
    assert_eq!(render_token_text(&"x".repeat(40)), "x".repeat(40));
    let long = format!("{}{}", "a".repeat(30), "b".repeat(30));
    assert_eq!(render_token_text(&long), format!("{}…{}", "a".repeat(20), "b".repeat(20)));
}

#[test]
fn metadata_rows_list_the_colors_their_contrast_and_the_font_styles() {
    let rows = metadata_rows("typescript", StandardTokenType::Comment, "#FFFFFF", "#000000", textmate::ITALIC | textmate::BOLD);
    let expected = [
        ("language", "typescript"),
        ("standard token type", "Comment"),
        ("foreground", "#FFFFFF"),
        ("background", "#000000"),
        ("contrast ratio", "21.00"),
        ("font style", "bold italic"),
    ];
    assert_eq!(rows, expected.map(|(key, value)| (key, value.to_string())));
}

#[test]
fn a_translucent_background_has_no_contrast_ratio_and_no_font_style_has_no_row() {
    let rows = metadata_rows("plaintext", StandardTokenType::Other, "#D4D4D4", "#1E1E1E80", 0);
    assert_eq!(rows[4], ("Contrast ratio cannot be precise for background colors that use transparency", String::new()));
    assert_eq!(rows.len(), 5);
}

#[test]
fn a_translucent_foreground_is_blended_onto_the_background_for_the_contrast_ratio() {
    let rows = metadata_rows("plaintext", StandardTokenType::Other, "#FFFFFF80", "#000000", 0);
    let blended = make_opaque(parse_hex("#FFFFFF80").unwrap(), parse_hex("#000000").unwrap());
    assert_eq!(blended, parse_hex("#808080").unwrap());
    assert_eq!(rows[4], ("contrast ratio", format!("{:.2}", contrast_ratio(blended, parse_hex("#000000").unwrap()))));
}

#[test]
fn the_matching_theme_rule_is_the_most_specific_selector_of_the_innermost_matching_scope() {
    let rules = [
        json!({ "settings": { "foreground": "#000000" } }),
        json!({ "scope": "comment", "settings": { "foreground": "#111111" } }),
        json!({ "scope": "source.ts comment.line, string", "settings": { "foreground": "#222222" } }),
        json!({ "scope": ["comment.line"], "settings": { "foreground": "#333333" } }),
        json!({ "scope": "source", "settings": { "fontStyle": "italic" } }),
    ];
    let comment = ["source.ts", "comment.line.double-slash.ts"];
    assert_eq!(find_matching_theme_rule(&rules, &comment), Some(("source.ts comment.line".into(), "{\n\t\"foreground\": \"#222222\"\n}".into())));
    let outside = ["source.js", "comment.block.js"];
    assert_eq!(find_matching_theme_rule(&rules, &outside).unwrap().0, "comment");
    let keyword = ["source.ts", "keyword.ts"];
    assert_eq!(find_matching_theme_rule(&rules, &keyword), Some(("source".into(), "{\n\t\"fontStyle\": \"italic\"\n}".into())));
    assert_eq!(find_matching_theme_rule(&rules, &["text.plain"]), None);
}

#[test]
fn of_equally_specific_selectors_the_later_rule_wins() {
    let rules = [json!({ "scope": "comment", "settings": { "foreground": "#111111" } }), json!({ "scope": "comment", "settings": { "foreground": "#222222" } })];
    assert_eq!(find_matching_theme_rule(&rules, &["comment"]).unwrap().1, "{\n\t\"foreground\": \"#222222\"\n}");
}

#[test]
fn a_typescript_comment_shows_its_scopes_innermost_first_and_the_theme_comment_rule() {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let extensions = exthost::scan::scan_builtin_extensions(&app_root.join("extensions")).unwrap();
    let languages = Languages::new(&extensions);
    let theme = load_theme(&extensions, "Dark Modern").unwrap();
    let path = std::env::temp_dir().join(format!("code-gpui-inspect-tokens-{}.ts", std::process::id()));
    std::fs::write(&path, "let a = 1;\n// note\n").unwrap();
    let document = TextDocument::read(&path, &languages, &crate::default_settings().lock().unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();

    let inspection = inspect(&document, Position::new(1, 4), &theme).unwrap();
    assert_eq!(inspection.text, "\u{00B7}note");
    assert_eq!(inspection.length, 5);
    assert_eq!(&inspection.metadata[..2], &[("language", "typescript".to_string()), ("standard token type", "Comment".to_string())]);
    assert_eq!(inspection.metadata[2], ("foreground", "#6A9955".to_string()));
    assert_eq!(inspection.scopes, ["comment.line.double-slash.ts", "source.ts"]);
    assert_eq!(inspection.theme_rule, Some(("comment".into(), "{\n\t\"foreground\": \"#6A9955\"\n}".into())));
}

#[gpui::test]
fn the_command_toggles_the_widget_and_escape_or_another_model_stops_it(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-inspect-tokens-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (first, second) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&first, "a b\n").unwrap();
    std::fs::write(&second, "c\n").unwrap();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx));
    let toggle = |cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("editor.action.inspectTMScopes", vec![], window, cx));
    let shown = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.inspect_tokens.shown.is_some());
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.open_file(first.clone(), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
    });
    cx.run_until_parked();

    toggle(cx);
    assert!(shown(cx));
    toggle(cx);
    assert!(!shown(cx), "the command toggles");
    toggle(cx);
    cx.simulate_event(KeyUpEvent { keystroke: gpui::Keystroke::parse("escape").unwrap() });
    assert!(!shown(cx), "Escape stops it");
    toggle(cx);
    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(second.clone(), true, window, cx));
    cx.run_until_parked();
    assert!(!shown(cx), "another model stops it");
    std::fs::remove_dir_all(&dir).unwrap();
}
