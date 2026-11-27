use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::editor::{EditReason, Position};
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

const DOCUMENT_HANDLE: u64 = 7;
const EVENT_HANDLE: u64 = 8;
const RANGE_HANDLE: u64 = 9;

/// `a.txt` open with semantic coloring on (the Dark Modern theme's `semanticHighlighting`), its provider registered.
fn open<'a>(cx: &'a mut TestAppContext, name: &str, method: &str, handle: u64) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-semantic-tokens-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "let x = 1;\nfoo();\n").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.open_file(dir.join("a.txt"), true, window, cx);
        let legend = json!({ "tokenTypes": ["variable", "function"], "tokenModifiers": ["readonly"] });
        workbench.language_features.register(method, vec![json!(handle), json!("*"), legend, json!(EVENT_HANDLE)]);
        workbench.semantic_tokens_model_changed(cx);
        workbench
    });
    cx.executor().advance_clock(DOCUMENT_DELAY);
    cx.run_until_parked();
    (workbench, cx, receiver)
}

/// The messages sent since the last call; the `method` calls are answered with `reply`.
fn answer(receiver: &Receiver<HostMessage>, method: &str, reply: Value, cx: &mut VisualTestContext) -> Vec<(&'static str, Vec<Value>)> {
    let mut sent = Vec::new();
    for message in receiver.try_iter() {
        sent.push((message.method, message.args.clone()));
        if message.method == method {
            let _ = message.reply.expect("a call").send(Ok(reply.clone()));
        }
    }
    cx.run_until_parked();
    sent
}

fn buffer(values: &[u32]) -> Value {
    json!(values.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<u8>>())
}

/// `let x` as a readonly variable, `foo` as a function.
fn full(id: u32) -> Value {
    buffer(&[id, 1, 10, 0, 4, 1, 0, 1, 1, 0, 3, 1, 0])
}

fn styles(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, line: usize) -> Vec<(Range<usize>, HighlightStyle)> {
    workbench.read_with(cx, |workbench, _| workbench.semantic_token_styles(workbench.editors.active().unwrap(), line, 4))
}

fn theme_style(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, token_type: &str, modifiers: &[&str]) -> HighlightStyle {
    workbench.read_with(cx, |workbench, _| highlight(&workbench.theme.semantic.get_token_style_metadata(token_type, modifiers, "plaintext")))
}

#[gpui::test]
fn full_tokens_are_styled_by_the_theme(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, "full", DOCUMENT_REGISTER, DOCUMENT_HANDLE);
    let sent = answer(&receiver, "$provideDocumentSemanticTokens", full(1), cx);
    let request = sent.iter().find(|(method, _)| *method == "$provideDocumentSemanticTokens").expect("the provider is asked");
    assert_eq!((&request.1[0], &request.1[2]), (&json!(DOCUMENT_HANDLE), &json!(0)));
    let readonly_variable = theme_style(&workbench, cx, "variable", &["readonly"]);
    let function = theme_style(&workbench, cx, "function", &[]);
    assert!(readonly_variable.color.is_some() && function.color.is_some() && readonly_variable.color != function.color);
    assert_eq!(styles(&workbench, cx, 0), [(4..5, readonly_variable)]);
    assert_eq!(styles(&workbench, cx, 1), [(0..3, function)]);
}

#[gpui::test]
fn a_refetch_sends_the_previous_result_id_and_applies_the_delta(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, "delta", DOCUMENT_REGISTER, DOCUMENT_HANDLE);
    answer(&receiver, "$provideDocumentSemanticTokens", full(1), cx);
    workbench.update(cx, |workbench, cx| workbench.semantic_tokens_event(EVENT_HANDLE, cx));
    cx.executor().advance_clock(DOCUMENT_DELAY);
    cx.run_until_parked();
    // `foo` becomes a plain variable: its type (index 8) and modifiers (index 9) are replaced.
    let delta = buffer(&[2, 2, 1, 8, 2, 2, 0, 0]);
    let sent = answer(&receiver, "$provideDocumentSemanticTokens", delta, cx);
    let request = sent.iter().find(|(method, _)| *method == "$provideDocumentSemanticTokens").expect("the provider is asked again");
    assert_eq!(request.1[2], json!(1));
    let released: Vec<Vec<Value>> = receiver.try_iter().filter(|message| message.method == "$releaseDocumentSemanticTokens").map(|message| message.args).collect();
    assert_eq!(released, [vec![json!(DOCUMENT_HANDLE), json!(1)]]);
    let variable = theme_style(&workbench, cx, "variable", &[]);
    assert_eq!(styles(&workbench, cx, 1), [(0..3, variable)]);
}

#[gpui::test]
fn a_range_reply_for_an_older_version_is_dropped(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, "stale", RANGE_REGISTER, RANGE_HANDLE);
    cx.executor().advance_clock(RANGE_DELAY);
    cx.run_until_parked();
    let requests: Vec<HostMessage> = receiver.try_iter().filter(|message| message.method == "$provideDocumentRangeSemanticTokens").collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].args[2]["endLineNumber"], json!(3));
    // The edit bumps the version without the hook that would cancel the request.
    workbench.update(cx, |workbench, _| {
        let at = Position::new(2, 0);
        workbench.editors.active_mut().unwrap().execute_edits(vec![(at, at, "z".into(), false)], None, EditReason::Unknown("test"));
    });
    for request in requests {
        let _ = request.reply.unwrap().send(Ok(full(1)));
    }
    cx.run_until_parked();
    assert_eq!(styles(&workbench, cx, 1), []);
    workbench.update(cx, |workbench, cx| workbench.semantic_tokens_providers_changed(cx));
    cx.executor().advance_clock(RANGE_DELAY);
    cx.run_until_parked();
    answer(&receiver, "$provideDocumentRangeSemanticTokens", full(1), cx);
    assert_eq!(styles(&workbench, cx, 1).len(), 1, "a reply for the current version is kept");
}

#[gpui::test]
fn turning_the_setting_off_clears_the_tokens_and_releases_the_result(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, "setting", DOCUMENT_REGISTER, DOCUMENT_HANDLE);
    answer(&receiver, "$provideDocumentSemanticTokens", full(1), cx);
    assert_eq!(styles(&workbench, cx, 1).len(), 1);
    workbench.update(cx, |workbench, cx| {
        workbench.settings.lock().unwrap().update_value_derived("editor.semanticHighlighting.enabled", Some(&json!(false))).unwrap();
        workbench.semantic_tokens_settings_changed(cx);
    });
    cx.executor().advance_clock(DOCUMENT_DELAY);
    let sent = answer(&receiver, "$provideDocumentSemanticTokens", full(2), cx);
    assert_eq!(styles(&workbench, cx, 1), []);
    assert!(sent.contains(&("$releaseDocumentSemanticTokens", vec![json!(DOCUMENT_HANDLE), json!(1)])));
    assert!(!sent.iter().any(|(method, _)| *method == "$provideDocumentSemanticTokens"), "no request while coloring is off");
}

#[gpui::test]
fn an_edit_moves_the_tokens(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx, "edit", DOCUMENT_REGISTER, DOCUMENT_HANDLE);
    answer(&receiver, "$provideDocumentSemanticTokens", full(1), cx);
    workbench.update(cx, |workbench, cx| {
        let at = Position::new(0, 0);
        let change = workbench.editors.active_mut().unwrap().execute_edits(vec![(at, at, "\n\t".into(), false)], None, EditReason::Unknown("test")).unwrap();
        workbench.semantic_tokens_content_changed(&change, cx);
    });
    let tab_expanded = styles(&workbench, cx, 1);
    assert_eq!(tab_expanded.iter().map(|(range, _)| range.clone()).collect::<Vec<_>>(), [8..9]);
    assert_eq!(styles(&workbench, cx, 2).len(), 1);
}

#[test]
fn a_typed_word_character_grows_the_token_it_touches() {
    let token = |line, start, end| Token { line, start, end, style: TokenStyle::default() };
    let change = |line, column, text: &str| ContentChange { start_line: line, start_column: column, end_line: line, end_column: column, range_offset: 0, range_length: 0, text: text.into() };
    let mut tokens = vec![token(0, 4, 7), token(1, 0, 3)];
    apply_content_change(&mut tokens, &change(1, 8, "a"));
    apply_content_change(&mut tokens, &change(2, 1, " "));
    assert_eq!(tokens, [token(0, 4, 8), token(1, 1, 4)]);
    let deletion = ContentChange { start_line: 1, start_column: 6, end_line: 2, end_column: 2, range_offset: 0, range_length: 0, text: String::new() };
    apply_content_change(&mut tokens, &deletion);
    assert_eq!(tokens, [token(0, 4, 5), token(0, 5, 8)]);
}

#[test]
fn the_overlay_replaces_only_the_properties_a_semantic_token_sets() {
    let red = Hsla::from(Rgba::try_from("#FF0000").unwrap());
    let blue = Hsla::from(Rgba::try_from("#0000FF").unwrap());
    let underline = UnderlineStyle { thickness: px(1.), color: Some(red), wavy: false };
    let base = HighlightStyle { color: Some(red), font_weight: Some(FontWeight::BOLD), underline: Some(underline), ..HighlightStyle::default() };
    let semantic_style = highlight(&TokenStyle { foreground: Some("#0000FF".into()), italic: Some(true), underline: Some(false), ..TokenStyle::default() });
    let overlaid = overlay_semantic_styles(vec![(0..10, base)], &[(2..5, semantic_style), (12..14, semantic_style)]);
    let merged = HighlightStyle { color: Some(blue), font_weight: Some(FontWeight::BOLD), font_style: Some(FontStyle::Italic), ..HighlightStyle::default() };
    let outside = HighlightStyle { color: Some(blue), font_style: Some(FontStyle::Italic), ..HighlightStyle::default() };
    assert_eq!(overlaid, [(0..2, base), (2..5, merged), (5..10, base), (12..14, outside)]);
}
