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

/// A folder with `a.ts` holding three lines, open, with one code lens provider.
fn open(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext, Receiver<HostMessage>) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-code-lens-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ts"), "function a() {}\n  function b() {}\nc\n").unwrap();
    let workspace = config::workspace::Workspace::folder(dir.clone());
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), Some(workspace.clone()))));
    let (sender, receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx);
        workbench.watch_editor_focus(window, cx);
        workbench.language_features.register(REGISTER, vec![json!(3), json!("*"), json!(9)]);
        workbench.open_file(dir.join("a.ts"), true, window, cx);
        window.focus(&workbench.editor_focus, cx);
        workbench
    });
    cx.executor().advance_clock(DELAY);
    cx.run_until_parked();
    (workbench, cx, receiver)
}

fn lens(line: u64, column: u64, command: Option<&str>) -> Value {
    let range = json!({ "startLineNumber": line, "startColumn": column, "endLineNumber": line, "endColumn": column + 1 });
    match command {
        Some(title) => json!({ "cacheId": [1, line], "range": range, "command": { "id": "lens.run", "title": title, "arguments": [line] } }),
        None => json!({ "cacheId": [1, line], "range": range }),
    }
}

/// Answers the requests of `method` sent since the last answer with `answer(args)`; returns their count.
fn reply(receiver: &Receiver<HostMessage>, method: &str, answer: impl Fn(&[Value]) -> Value, cx: &mut VisualTestContext) -> usize {
    let mut count = 0;
    for message in receiver.try_iter().filter(|message| message.method == method) {
        count += 1;
        let _ = message.reply.expect("a call").send(Ok(answer(&message.args)));
    }
    cx.run_until_parked();
    count
}

#[gpui::test]
fn lenses_get_zones_above_their_lines_and_resolve_in_view(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    let lenses = json!({ "cacheId": 4, "lenses": [lens(2, 3, None), lens(1, 1, Some("1 reference")), lens(1, 10, None)] });
    assert_eq!(reply(&receiver, "$provideCodeLenses", |_| lenses.clone(), cx), 1);
    workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().unwrap();
        let lines: Vec<usize> = workbench.code_lenses.groups.iter().map(|group| editor.view_zone(group.zone_id).unwrap().anchor.unwrap().line).collect();
        assert_eq!(lines, [0, 1]);
        // The first group shows its lens with a command at once; the second waits for its resolve.
        assert_eq!(workbench.code_lenses.groups[0].shown.as_ref().map(Vec::len), Some(1));
        assert!(workbench.code_lenses.groups[1].shown.is_none());
        assert!(editor.line_top(0, workbench.editors.viewport) > 0.);
    });
    cx.executor().advance_clock(DELAY);
    cx.run_until_parked();
    let resolved = reply(&receiver, "$resolveCodeLens", |args| {
        let mut symbol = args[1].clone();
        symbol["command"] = json!({ "id": "", "title": "$(gear) resolved" });
        symbol
    }, cx);
    assert_eq!(resolved, 2);
    workbench.read_with(cx, |workbench, _| {
        let shown = workbench.code_lenses.groups[1].shown.as_ref().unwrap();
        assert_eq!(shown[0].as_ref().unwrap()["command"]["title"], "$(gear) resolved");
        assert_eq!(workbench.code_lenses.groups[0].shown.as_ref().unwrap().len(), 2);
    });
    // An edit asks again; lenses on the same lines keep their zones.
    let zones: Vec<u32> = workbench.read_with(cx, |workbench, _| workbench.code_lenses.groups.iter().map(|group| group.zone_id).collect());
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("type", vec![json!({ "text": "x" })], window, cx));
    cx.executor().advance_clock(DELAY);
    cx.run_until_parked();
    assert_eq!(reply(&receiver, "$provideCodeLenses", |_| lenses.clone(), cx), 1);
    let released: Vec<Value> = receiver.try_iter().filter(|message| message.method == "$releaseCodeLenses").map(|message| message.args[1].clone()).collect();
    assert_eq!(released, [json!(4)]);
    workbench.read_with(cx, |workbench, _| assert_eq!(workbench.code_lenses.groups.iter().map(|group| group.zone_id).collect::<Vec<_>>(), zones));
}

#[gpui::test]
fn the_current_line_command_picks_among_its_lenses(cx: &mut TestAppContext) {
    let (workbench, cx, receiver) = open(cx);
    reply(&receiver, "$provideCodeLenses", |_| json!({ "lenses": [lens(1, 1, Some("run")), lens(1, 5, Some("debug")), lens(2, 1, Some("other"))] }), cx);
    workbench.update_in(cx, |workbench, window, cx| _ = workbench.execute_command("codelens.showLensesInCurrentLine", vec![], window, cx));
    cx.run_until_parked();
    workbench.read_with(cx, |workbench, _| {
        let input = workbench.quick_inputs.current().expect("a pick shows");
        let labels: Vec<_> = input.items().iter().filter_map(|entry| match entry {
            Entry::Item(item) => Some(item.label.clone()),
            _ => None,
        }).collect();
        assert_eq!(labels, ["run", "debug"]);
    });
}
