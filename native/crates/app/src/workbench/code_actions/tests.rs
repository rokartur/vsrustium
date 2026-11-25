use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use exthost::scan::scan_extension;
use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;

fn item(action: Value) -> CodeActionItem {
    CodeActionItem { action, handle: 1 }
}

#[test]
fn kinds_contain_their_children_and_the_empty_kind_contains_all() {
    assert!(contains("refactor", "refactor.extract"));
    assert!(contains("refactor", "refactor"));
    assert!(!contains("refactor", "refactorx"));
    assert!(contains("", "quickfix"));
    assert!(!contains(NONE_KIND, "quickfix"));
}

#[test]
fn filters_drop_source_actions_unless_asked_and_keep_preferred_only_when_asked() {
    let fix = json!({ "title": "a", "kind": "quickfix" });
    let organize = json!({ "title": "b", "kind": "source.organizeImports" });
    let plain = json!({ "title": "c" });
    assert!(filters_action(&Filter::default(), &fix));
    assert!(!filters_action(&Filter::default(), &organize));
    assert!(filters_action(&Filter { include_source_actions: true, ..Default::default() }, &organize));
    // An include drops actions without a kind, even the empty kind of `editor.action.codeAction`.
    assert!(!filters_action(&Filter { include: Some(String::new()), ..Default::default() }, &plain));
    assert!(!filters_action(&Filter { only_preferred: true, ..Default::default() }, &fix));
    assert!(!may_include_kind(&Filter { include: Some("refactor".into()), ..Default::default() }, "quickfix"));
    assert!(may_include_kind(&Filter { include: Some("refactor.extract".into()), ..Default::default() }, "refactor"));
    assert!(!may_include_kind(&Filter { excludes: vec!["notebook".into()], ..Default::default() }, "notebook.format"));
}

#[test]
fn sets_put_diagnostic_fixes_first_then_preferred_and_ai_last() {
    let set = CodeActionSet::new(
        vec![
            item(json!({ "title": "ai", "isAI": true, "diagnostics": [{}] })),
            item(json!({ "title": "plain" })),
            item(json!({ "title": "preferred", "isPreferred": true })),
            item(json!({ "title": "fix", "kind": "quickfix", "diagnostics": [{}] })),
            item(json!({ "title": "off", "disabled": "no" })),
        ],
        Vec::new(),
        Vec::new(),
    );
    let titles: Vec<&str> = set.all.iter().map(|item| item.action["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["fix", "preferred", "plain", "off", "ai"]);
    assert_eq!(set.valid.len(), 4);
    assert!(!set.has_auto_fix);
    assert!(set.has_ai_fix);
    assert!(!set.all_ai_fixes);
}

#[test]
fn command_args_read_kind_apply_and_preferred() {
    let (kind, apply, preferred) = command_args(Some(&json!({ "kind": "refactor.extract", "apply": "First", "preferred": true })), "", AutoApply::IfSingle);
    assert_eq!(kind, "refactor.extract");
    assert!(apply == AutoApply::First);
    assert!(preferred);
    assert_eq!(none_message(Some(&json!({ "kind": "refactor.extract" })), false, "refactorings"), "No refactorings for 'refactor.extract' available");
    assert_eq!(none_message(None, true, "code actions"), "No preferred code actions available");
}

#[test]
fn indent_levels_and_keybindings_of_kinds() {
    assert_eq!(compute_indent_level("\t  x", 4), 6);
    assert_eq!(compute_indent_level("   ", 4), -1);
    let bindings = vec![("source".to_owned(), false, "A".to_owned()), ("source.organizeImports".to_owned(), false, "B".to_owned())];
    assert_eq!(keybinding_for_action(&bindings, &json!({ "kind": "source.organizeImports" })).as_deref(), Some("B"));
    assert_eq!(keybinding_for_action(&bindings, &json!({ "kind": "source.fixAll" })).as_deref(), Some("A"));
    assert_eq!(keybinding_for_action(&bindings, &json!({ "kind": "quickfix" })), None);
}

/// A folder with `a.ts` holding `text`, open with the cursor at its start, with one code action provider.
fn open<'a>(text: &str, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-code-actions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ts"), text).unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.language_features.register(REGISTER, vec![json!(7), json!("*"), json!({}), json!("test"), json!("test.ext"), json!(false)]);
        workbench.open_file(dir.join("a.ts"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench
    });
    cx.run_until_parked();
    (workbench, cx, receiver)
}

/// Answers the code action requests sent since the last answer; returns their contexts.
fn answer(receiver: &Receiver<HostMessage>, actions: Value, cx: &mut VisualTestContext) -> Vec<Value> {
    let mut contexts = Vec::new();
    for message in receiver.try_iter().filter(|message| message.method == "$provideCodeActions") {
        contexts.push(message.args[3].clone());
        let _ = message.reply.expect("a call").send(Ok(json!({ "cacheId": contexts.len(), "actions": actions })));
    }
    cx.run_until_parked();
    contexts
}

fn edit_action(title: &str, text: &str) -> Value {
    let path = std::env::temp_dir().join(format!("code-gpui-code-actions-{}", std::process::id())).join("a.ts");
    let resource = json!({ "scheme": "file", "path": path.to_string_lossy() });
    json!({ "title": title, "kind": "quickfix", "edit": { "edits": [{ "resource": resource, "textEdit": { "range": { "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 1 }, "text": text } }] } })
}

#[gpui::test]
fn an_automatic_request_shows_the_light_bulb_and_cmd_period_applies_from_the_menu(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open("let x\n", cx);
    let contexts = answer(&receiver, json!([edit_action("Add a", "a"), edit_action("Add b", "b")]), cx);
    assert_eq!(contexts, [json!({ "only": null, "trigger": AUTO })]);
    workbench.read_with(cx, |workbench, _| {
        let lightbulb = workbench.code_actions.lightbulb.as_ref().expect("two actions show the light bulb");
        // `let x` has no indentation to hold it, the empty line below does.
        assert_eq!((lightbulb.line, lightbulb.column), (1, Some(0)));
    });
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("editor.action.quickFix", vec![], window, cx));
    cx.run_until_parked();
    let contexts = answer(&receiver, json!([edit_action("Add a", "a"), edit_action("Add b", "b")]), cx);
    assert_eq!(contexts, [json!({ "only": null, "trigger": INVOKE })]);
    workbench.update_in(cx, |workbench, window, cx| {
        assert!(workbench.action_widget_visible());
        assert_eq!(workbench.action_widget_labels(), ["Quick Fix", "Add a", "Add b"]);
        _ = workbench.execute_command("selectNextCodeAction", vec![], window, cx);
        _ = workbench.execute_command("acceptSelectedCodeAction", vec![], window, cx);
    });
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        assert!(!workbench.action_widget_visible());
        assert_eq!(workbench.editors.active().unwrap().document.lines[0], "blet x");
    });
    let released: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$releaseCodeActions").map(|message| message.args[1].clone()).collect();
    assert!(released.contains(&json!(1)), "the replaced automatic list is released: {released:?}");
}

#[gpui::test]
fn the_marker_hover_asks_for_the_markers_quick_fixes(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open("let x\n", cx);
    answer(&receiver, json!([]), cx);
    let path = std::env::temp_dir().join(format!("code-gpui-code-actions-{}", std::process::id())).join("a.ts");
    let uri = json!({ "scheme": "file", "path": path.to_string_lossy() });
    let marker = json!({ "severity": 8, "message": "bad", "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 4 });
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.change_many_markers("test", &json!([[uri, [marker]]]), cx);
        _ = workbench.execute_command("editor.action.showHover", vec![], window, cx);
    });
    cx.run_until_parked();
    let contexts = answer(&receiver, json!([edit_action("Fix", "f")]), cx);
    assert!(contexts.contains(&json!({ "only": "quickfix", "trigger": INVOKE })), "{contexts:?}");
    workbench.update_in(cx, |workbench, _, cx| {
        let marker = workbench.code_actions.marker_fixes.marker.clone().expect("the hover shows the marker");
        assert!(matches!(workbench.marker_quick_fix_status(&marker), QuickFixStatus::QuickFix));
        workbench.show_marker_quick_fixes(cx);
        assert_eq!(workbench.action_widget_labels(), ["Quick Fix", "Fix"]);
    });
}

#[gpui::test]
fn a_kind_command_applies_a_single_action_and_reports_none(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open("let x\n", cx);
    answer(&receiver, json!([]), cx);
    let organize = json!({ "kind": "source.organizeImports" });
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("editor.action.codeAction", vec![organize], window, cx));
    cx.run_until_parked();
    let contexts = answer(&receiver, json!([{ "title": "Organize", "kind": "source.organizeImports", "edit": edit_action("", "o")["edit"] }]), cx);
    assert_eq!(contexts, [json!({ "only": "source.organizeImports", "trigger": INVOKE })]);
    workbench.read_with(cx, |workbench, _| {
        assert!(!workbench.action_widget_visible());
        assert_eq!(workbench.editors.active().unwrap().document.lines[0], "olet x");
    });
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("editor.action.fixAll", vec![], window, cx));
    cx.run_until_parked();
    answer(&receiver, json!([]), cx);
    workbench.read_with(cx, |workbench, _| assert!(workbench.editor_message_visible()));
}
