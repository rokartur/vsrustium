use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

fn workbench(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, std::sync::mpsc::Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let instance = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vsrustium-playground-{}-{instance}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("User")).unwrap();
    let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
    let extensions: Vec<_> = ["css", "javascript", "json", "html"].iter().map(|name| scan_extension(&app_root.join("extensions").join(name), true).unwrap()).collect();
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) =
        cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&extensions), Arc::new(Mutex::new(settings)), HostSync::new(sender), cx));
    workbench.update_in(cx, |workbench, _, cx| workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("User/keybindings.json"), cx));
    run(&workbench, "workbench.action.showInteractivePlayground", cx);
    (workbench, cx, receiver)
}

fn run(workbench: &gpui::Entity<Workbench>, id: &str, cx: &mut VisualTestContext) {
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command(id, Vec::new(), window, cx)));
    cx.run_until_parked();
}

#[gpui::test]
fn the_playground_opens_one_editor_per_code_block_in_its_language(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = workbench(cx);
    workbench.read_with(cx, |workbench, _| {
        let tab = workbench.editors.active_tab().unwrap();
        assert_eq!(workbench.editor_name(tab), "Editor Playground");
        let snippets: Vec<(String, String)> = workbench.snippet_models().iter().map(|model| (crate::editor::split_fragment(SNIPPET_SCHEME, &model.document().path).1.to_owned(), model.document().language_id.clone())).collect();
        let expected = [("1.css", "css"), ("2.js", "javascript"), ("3.json", "json"), ("4.js", "javascript"), ("5.js", "javascript"), ("6.html", "html"), ("7.js", "javascript"), ("8.js", "javascript"), ("9.html", "html"), ("10.js", "javascript")];
        assert_eq!(snippets, expected.map(|(fragment, language)| (fragment.to_owned(), language.to_owned())));
        assert_eq!(workbench.snippet_models()[2].document().lines[1], "\t\"name\": \"John\",");
        assert!(!workbench.snippet_models()[0].is_dirty());
    });
    // mainThreadDocumentsAndEditors.ts: each snippet is a document and a visible text editor of the host.
    let deltas: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$acceptDocumentsAndEditorsDelta").map(|message| message.args[0].clone()).collect();
    let documents: Vec<&Value> = deltas.iter().flat_map(|delta| delta["addedDocuments"].as_array().into_iter().flatten()).filter(|document| document["uri"]["scheme"] == SNIPPET_SCHEME).collect();
    assert_eq!(documents.len(), 10);
    let uri = &documents[0]["uri"];
    assert_eq!((uri["authority"].as_str(), uri["fragment"].as_str()), (Some("vscode-app"), Some("1.css")));
    assert!(uri["path"].as_str().unwrap().ends_with("/out/vs/workbench/contrib/welcomeWalkthrough/browser/editor/vs_code_editor_walkthrough.md"));
    assert_eq!(uri["query"], json!({ "moduleId": MODULE_ID }).to_string());
    let editors = deltas.iter().flat_map(|delta| delta["addedEditors"].as_array().into_iter().flatten()).filter(|editor| editor["documentUri"]["scheme"] == SNIPPET_SCHEME).count();
    assert_eq!(editors, 10);
}

#[gpui::test]
fn the_page_has_the_focus_until_a_snippet_is_pressed(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench(cx);
    workbench.update_in(cx, |workbench, window, _| {
        let context = workbench.key_context(window);
        assert_eq!(context.get("interactivePlaygroundFocus"), Some(&json!(true)));
        assert_eq!(context["activeEditor"], "workbench.editor.walkThroughPart");
        assert_eq!(context["editorTextFocus"], false);
    });
    workbench.update_in(cx, |workbench, window, cx| {
        let path = workbench.snippet_models()[1].document().path.clone();
        workbench.focus_snippet(path, window, cx);
    });
    cx.run_until_parked();
    workbench.update_in(cx, |workbench, window, _| {
        let context = workbench.key_context(window);
        assert_eq!(context.get("interactivePlaygroundFocus"), None);
        assert_eq!(context["editorTextFocus"], true);
        assert_eq!(context["activeEditor"], "workbench.editor.walkThroughPart");
        assert_eq!(workbench.editors.active().unwrap().document().language_id, "javascript");
    });
    // Typing edits the focused snippet; the tab stays clean.
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("type", vec![json!({ "text": "x" })], window, cx)));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.snippet_models()[1].document().lines[0], "xconst canvas = document.querySelector('canvas');");
        assert!(!workbench.tab_shows_dirty(0));
    });
}

#[gpui::test]
fn the_arrow_and_page_commands_scroll_the_page(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench(cx);
    let top = |workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.playground_scroll_top());
    run(&workbench, "workbench.action.interactivePlayground.arrowDown", cx);
    run(&workbench, "workbench.action.interactivePlayground.arrowDown", cx);
    assert_eq!(top(&workbench, cx), 72.);
    run(&workbench, "workbench.action.interactivePlayground.arrowUp", cx);
    assert_eq!(top(&workbench, cx), 36.);
    let height = workbench.read_with(cx, |workbench, _| f32::from(workbench.playground.scroll.bounds().size.height));
    assert!(height > 0.);
    run(&workbench, "workbench.action.interactivePlayground.pageDown", cx);
    assert_eq!(top(&workbench, cx), 36. + height);
    run(&workbench, "workbench.action.interactivePlayground.pageUp", cx);
    run(&workbench, "workbench.action.interactivePlayground.pageUp", cx);
    assert_eq!(top(&workbench, cx), 0.);
}

#[gpui::test]
fn a_heading_link_scrolls_its_heading_20px_below_the_top(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench(cx);
    workbench.update_in(cx, |workbench, window, cx| workbench.open_playground_link("#line-actions", window, cx));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        let blocks = parse(CONTENT);
        let heading = blocks.iter().position(|block| matches!(block, Block::Heading(_, inline) if inline.text == "Line Actions")).unwrap();
        let bounds = workbench.playground.scroll.bounds_for_item(heading + 1).unwrap();
        let top = f32::from(bounds.top() - workbench.playground.scroll.bounds().top());
        assert_eq!(workbench.playground_scroll_top(), top - 20.);
    });
}

#[gpui::test]
fn a_closed_playground_comes_back_at_its_scroll_position(cx: &mut TestAppContext) {
    let (workbench, cx, _) = workbench(cx);
    run(&workbench, "workbench.action.interactivePlayground.pageDown", cx);
    let top = workbench.read_with(cx, |workbench, _| workbench.playground_scroll_top());
    assert!(top > 0.);
    run(&workbench, "workbench.action.closeActiveEditor", cx);
    workbench.update(cx, |workbench, _| workbench.playground.scroll.set_offset(point(px(0.), px(0.))));
    run(&workbench, "workbench.action.showInteractivePlayground", cx);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.playground_scroll_top()), top);
}

#[test]
fn kb_macros_become_shortcuts_or_unbound() {
    let label = |command: &str| (command == "editor.action.rename").then(|| "F2".to_owned());
    assert_eq!(expand_macros("Hit kb(editor.action.rename) or kb(no.such-command).", label), "Hit `` F2 `` or `` unbound ``.");
    assert_eq!(expand_macros("kb() and kb(x y)", label), "kb() and kb(x y)");
}

#[test]
fn spans_render_as_on_macos() {
    let line = "press <span class=\"mac-only windows-only\">any of `` ⇧↓ ``. Also</span> <span class=\"shortcut mac-only\">`⇧⌥`</span><span class=\"shortcut windows-only linux-only\">`Shift+Alt`</span> while";
    assert_eq!(expand_spans(line, "⌥"), "press any of `` ⇧↓ ``. Also `⇧⌥` while");
    let modifier = "with <span class=\"shortcut\"><span class=\"multi-cursor-modifier\"></span>+Click</span> to";
    assert_eq!(expand_spans(modifier, "⌥"), "with `` ⌥+Click `` to");
    assert_eq!(expand_spans("a <span class=\"linux-only\">b</span>c", "⌥"), "a c");
}

#[test]
fn command_links_render_as_text() {
    let blocks = parse("our [keyboard reference guide](command:workbench.action.keybindingsReference) and [docs](https://code.visualstudio.com/docs)");
    let Block::Paragraph(inline) = &blocks[0] else { panic!("a paragraph") };
    let links: Vec<_> = inline.spans.iter().filter_map(|span| span.link.clone()).collect();
    assert_eq!(links, ["https://code.visualstudio.com/docs"]);
}
