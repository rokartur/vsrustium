use std::path::Path;
use std::sync::{Arc, Mutex};

use exthost::scan::scan_extension;
use gpui::TestAppContext;

use crate::editor::{Position, Selection};
use crate::explorer::FileTree;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;
use storage::Storage;

fn labels(workbench: &Workbench, _: &gpui::App) -> Vec<String> {
    let input = workbench.quick_inputs.current().expect("a visible quick input");
    input.order.iter().map(|row| &input.rows[*row]).map(|row| format!("{}{}", row.separator.as_ref().map(|label| format!("--{label}--")).unwrap_or_default(), row.item.label)).collect()
}

/// A row as the reference capture prints it: `--separator--label [description] {label highlights} {d:description highlights}`.
fn reference_rows(workbench: &Workbench, _: &gpui::App) -> Vec<String> {
    let input = workbench.quick_inputs.current().expect("a visible quick input");
    let highlighted = |text: &str, ranges: &[std::ops::Range<usize>]| {
        let chars: Vec<char> = text.chars().collect();
        let parts: Vec<String> = ranges.iter().map(|range| chars[range.start.min(chars.len())..range.end.min(chars.len())].iter().collect()).collect();
        parts.join("|")
    };
    let mut rows = Vec::new();
    for row in input.order.iter().map(|row| &input.rows[*row]) {
        let mut text = row.separator.as_ref().map(|label| format!("--{label}--")).unwrap_or_default() + &row.item.label;
        let description = row.item.description.clone().unwrap_or_default();
        if !description.is_empty() {
            text += &format!(" [{description}]");
        }
        if !row.label_highlights.is_empty() {
            text += &format!(" {{{}}}", highlighted(&row.item.label, &row.label_highlights));
        }
        if !row.description_highlights.is_empty() {
            text += &format!(" {{d:{}}}", highlighted(&description, &row.description_highlights));
        }
        rows.push(text);
    }
    rows
}

/// The reference (1.147.0 on the Studio) over port/fixtures/workspace, opened with docs/guide.md, driven through CDP:
/// Cmd+P, the query typed, the rows read from the DOM.
#[gpui::test]
fn quick_open_shows_the_rows_of_the_reference(cx: &mut TestAppContext) {
    let reference: &[(&str, &[&str])] = &[
        ("", &["--recently opened--guide.md [docs]"]),
        ("util", &["--file results--util.ts [src] {util}"]),
        ("ts", &["--file results--main.ts [src] {ts}", "util.ts [src] {ts}", "notes.txt {t|s}"]),
        ("main", &["--file results--main.ts [src] {main}"]),
        ("md", &["--recently opened--guide.md [docs] {md}", "--file results--README.md {md}"]),
        ("src/", &["--file results--main.ts [src] {d:src}", "util.ts [src] {d:src}"]),
        ("s/m", &["--file results--main.ts [src] {m} {d:s|}", "guide.md [docs] {m} {d:s}"]),
        ("src/util", &["--file results--util.ts [src] {util} {d:src}"]),
        ("zzz", &["No matching results"]),
        ("main.ts:2:3", &["--file results--main.ts [src] {main.ts}"]),
        ("guide", &["--recently opened--guide.md [docs] {guide}"]),
        ("notes txt", &["--file results--notes.txt {notes|txt}"]),
        ("READ", &["--file results--README.md {READ}"]),
        ("pkg", &["--file results--package.json {p|k|g}"]),
        ("json", &["--file results--package.json {json}"]),
        ("\"main\"", &["--file results--main.ts [src] {main}"]),
        ("docs guide", &["--file results--guide.md [docs] {guide} {d:docs}"]),
        ("ut ts", &["--file results--util.ts [src] {ut|ts}"]),
        ("./src/main.ts", &["No matching results"]),
        ("src\\main", &["--file results--main.ts [src] {main} {d:src}"]),
        ("mn", &["--file results--main.ts [src] {m|n}"]),
    ];
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let folder = app_root.join("port/fixtures/workspace");
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(FileTree::new(folder.clone())), Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), app_root.join("keybindings-unused.json"), cx);
        workbench.open_file(folder.join("docs/guide.md"), true, window, cx);
        workbench
    });
    cx.run_until_parked();
    let mut mismatches = Vec::new();
    for (query, expected) in reference {
        cx.simulate_keystrokes("cmd-p");
        cx.simulate_input(query);
        cx.run_until_parked();
        let rows = workbench.read_with(cx, reference_rows);
        if rows != *expected {
            mismatches.push(format!("{query:?}: port {rows:?}, reference {expected:?}"));
        }
        cx.simulate_keystrokes("escape");
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[gpui::test]
fn quick_open_lists_history_and_files_and_opens_at_the_line(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let folder = app_root.join("port/fixtures/workspace");
    let storage = Arc::new(Mutex::new(Storage::in_memory()));
    let guide = format!(r#"[{{"editor":{{"resource":"file://{}/docs/guide.md"}}}}]"#, folder.display());
    storage.lock().unwrap().set("history.entries", guide);
    let workspace = storage.clone();
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), Some(FileTree::new(folder.clone())), Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), app_root.join("keybindings-unused.json"), cx);
        workbench.restore_workspace_state(workspace);
        workbench
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    let placeholder = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().placeholder.clone());
    assert_eq!(placeholder.as_deref(), Some("Search files by name (append : to go to line or @ to go to symbol), or paste a link"));
    assert_eq!(workbench.read_with(cx, labels), ["--recently opened--guide.md"]);

    cx.simulate_input("util");
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, labels), ["--file results--util.ts"]);

    cx.simulate_input("zzz");
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, labels), ["No matching results"]);
    cx.simulate_keystrokes("enter");
    assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_some()), "no matching results keeps the picker");
    cx.simulate_keystrokes("escape");

    workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("workbench.action.quickOpen", vec!["ts".into()], window, cx));
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, labels), ["--file results--main.ts", "util.ts", "notes.txt"], "a value given before the files load");
    cx.simulate_keystrokes("escape");

    cx.simulate_keystrokes("cmd-p");
    cx.simulate_input("main.ts:2:3");
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, labels), ["--file results--main.ts"]);
    cx.simulate_keystrokes("enter");
    let (path, selection, preview) = workbench.read_with(cx, |workbench, _| {
        let editor = workbench.editors.active().expect("an opened editor");
        (editor.document.path.clone(), editor.selection, editor.preview)
    });
    assert_eq!(path, folder.join("src/main.ts"));
    assert_eq!(selection, Selection::caret(Position::new(1, 2)));
    assert!(!preview, "Quick Open opens pinned without workbench.editor.enablePreviewFromQuickOpen");
    let stored = storage.lock().unwrap().get("history.entries").map(str::to_owned).unwrap();
    assert!(stored.starts_with(&format!(r#"[{{"editor":{{"resource":"file://{}/src/main.ts""#, folder.display())), "{stored}");

    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, labels), ["--recently opened--main.ts", "guide.md"]);
    cx.simulate_input("gui");
    cx.run_until_parked();
    assert_eq!(workbench.read_with(cx, labels), ["--recently opened--guide.md"], "history entries are not repeated as file results");
}
