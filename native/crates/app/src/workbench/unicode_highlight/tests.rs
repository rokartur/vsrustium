use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::{unicode_exclude_options, unicode_hover_reason};
use crate::editor::unicode_highlighter::{CodePointHighlighter, HighlightReason, UnicodeHighlighterOptions};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn the_banner_action_takes_the_focus_out_of_the_editor_with_the_banner(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-unicode-banner-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("many.ts");
    std::fs::write(&file, format!("let a = {}\nlet b", "x\u{430}".repeat(1100))).unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.open_file(file.clone(), true, window, cx);
        // Languages::new(&[]) knows no TypeScript; any language but plaintext keeps the ambiguous highlight on.
        workbench.editors.active_mut().unwrap().document_mut().language_id = "typescript".into();
        workbench
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let highlights = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().highlights.len());
    assert_eq!(highlights(cx), 2);

    let link = cx.debug_bounds("unicode-banner-action").expect("the banner shows past 1000 characters");
    cx.simulate_click(link.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("unicode-banner-action").is_none(), "the highlight is off, so is the banner");
    assert_eq!(highlights(cx), 0, "onDidBlurEditorWidget");
    let focused = workbench.update_in(cx, |workbench, window, cx| workbench.editor_focus.contains_focused(window, cx));
    assert!(!focused);
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui::test]
fn non_basic_ascii_defaults_on_only_in_an_untrusted_workspace(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-unicode-trust-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.ts");
    std::fs::write(&file, "let caf\u{e9} = 1").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.open_file(file.clone(), true, window, cx);
        workbench
    });
    let non_basic_ascii = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.unicode_highlighter_options(workbench.editors.active().unwrap()).is_some_and(|options| options.non_basic_ascii));
    assert!(!non_basic_ascii(cx), "trusted: `inUntrustedWorkspace` is off");
    let mut storage = storage::Storage::in_memory();
    let startup = crate::workspace_trust::Startup { enabled: true, workspace: None, empty_window_trusted: false, workspace_storage: &mut storage };
    let untrusted = crate::workspace_trust::WorkspaceTrust::new(Arc::new(Mutex::new(storage::Storage::in_memory())), startup);
    assert!(!untrusted.is_trusted());
    workbench.update(cx, |workbench, _| workbench.workspace_trust = untrusted);
    assert!(non_basic_ascii(cx), "untrusted: `inUntrustedWorkspace` is on");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn hover_reasons_read_as_the_reference_writes_them() {
    let options = UnicodeHighlighterOptions { ambiguous_characters: true, invisible_characters: true, allowed_locales: vec!["en".into()], ..Default::default() };
    let mut highlighter = CodePointHighlighter::new(&options);
    assert_eq!(
        unicode_hover_reason('\u{430}', HighlightReason::Ambiguous, &highlighter),
        "The character `U+0430` \"`\u{430}`\" could be confused with the ASCII character `U+0061` \"`a`\", which is more common in source code."
    );
    // No upstream confusable maps to a non-ASCII char yet; the branch stays for data that adds one.
    highlighter.ambiguous.insert(0x2013, 0xe9);
    assert_eq!(
        unicode_hover_reason('\u{2013}', HighlightReason::Ambiguous, &highlighter),
        "The character `U+2013` \"`\u{2013}`\" could be confused with the character `U+00e9` \"`\u{e9}`\", which is more common in source code."
    );
    assert_eq!(unicode_hover_reason('\u{200b}', HighlightReason::Invisible, &highlighter), "The character `U+200b` is invisible.");
    assert_eq!(unicode_hover_reason('\u{e9}', HighlightReason::NonBasicAscii, &highlighter), "The character `U+00e9` \"`\u{e9}`\" is not a basic ASCII character.");
    assert_eq!(unicode_hover_reason('`', HighlightReason::NonBasicAscii, &highlighter), "The character `U+0060` \"`` ` ``\" is not a basic ASCII character.");
}

#[test]
fn exclude_options_follow_the_reference_order_and_writes() {
    let args = json!({ "codePoint": 0x430, "reason": { "kind": 0, "confusableWith": "a", "notAmbiguousInLocales": ["ru"] }, "inComment": true, "inString": false });
    let options = unicode_exclude_options(&args, Some(json!({ "de": true })), json!({ "\u{e9}": true })).unwrap();
    assert_eq!(
        options,
        vec![
            ("Allow unicode characters that are more common in the language \"ru\".".into(), "editor.unicodeHighlight.allowedLocales", json!({ "de": true, "ru": true })),
            ("Exclude U+0430 \"\u{430}\" from being highlighted".into(), "editor.unicodeHighlight.allowedCharacters", json!({ "\u{e9}": true, "\u{430}": true })),
            ("Disable highlighting of characters in comments".into(), "editor.unicodeHighlight.includeComments", json!(false)),
            ("Disable highlighting of ambiguous characters".into(), "editor.unicodeHighlight.ambiguousCharacters", json!(false)),
        ]
    );

    let args = json!({ "codePoint": 0x200b, "reason": { "kind": 1 }, "inComment": false, "inString": true });
    let options = unicode_exclude_options(&args, None, Value::Null).unwrap();
    assert_eq!(
        options,
        vec![
            ("Exclude U+200b (invisible character) from being highlighted".into(), "editor.unicodeHighlight.allowedCharacters", json!({ "\u{200b}": true })),
            ("Disable highlighting of characters in strings".into(), "editor.unicodeHighlight.includeStrings", json!(false)),
            ("Disable highlighting of invisible characters".into(), "editor.unicodeHighlight.invisibleCharacters", json!(false)),
        ]
    );

    let args = json!({ "codePoint": 0xe9, "reason": { "kind": 2 }, "inComment": false, "inString": false });
    let labels: Vec<_> = unicode_exclude_options(&args, None, Value::Null).unwrap().into_iter().map(|(label, key, _)| (label, key)).collect();
    assert_eq!(
        labels,
        vec![
            ("Exclude U+00e9 \"\u{e9}\" from being highlighted".to_string(), "editor.unicodeHighlight.allowedCharacters"),
            ("Disable highlighting of non basic ASCII characters".into(), "editor.unicodeHighlight.nonBasicASCII"),
        ]
    );
}
