use std::path::Path;

use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::menus::MenuAction;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn titles(actions: &[MenuAction]) -> Vec<String> {
    let mut titles = Vec::new();
    for action in actions {
        match action {
            MenuAction::Command { title, checked, .. } => titles.push(format!("{title}{}", if *checked { " *" } else { "" })),
            MenuAction::Submenu { title, actions, .. } => titles.push(format!("{title} [{}]", titles_joined(actions))),
            MenuAction::Separator => titles.push("-".into()),
        }
    }
    titles
}

fn titles_joined(actions: &[MenuAction]) -> String {
    titles(actions).join(", ")
}

#[gpui::test]
fn markdown_editor_files_offer_their_editors_in_the_breadcrumbs(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let markdown = scan_extension(&app_root.join("extensions/markdown-language-features"), true).unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-editor-type-picker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, code) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.rs"));
    std::fs::write(&a, "one\n").unwrap();
    std::fs::write(&b, "two\n").unwrap();
    std::fs::write(&code, "fn main() {}\n").unwrap();
    let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
    let (workbench, cx) = cx.add_window_view(|_, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(std::sync::mpsc::channel().0), cx);
        workbench.extensions = vec![markdown];
        workbench
    });
    let menu = |cx: &mut gpui::VisualTestContext| {
        workbench.update_in(cx, |workbench, window, cx| {
            let available = workbench.available_editor_types().map(|available| available.current_label());
            workbench.show_editor_type_picker(gpui::Bounds::default(), window, cx);
            let actions = workbench.context_menu.as_ref().map(|menu| titles(&menu.levels[0].actions)).unwrap_or_default();
            workbench.hide_context_menu(window, cx);
            (available, actions)
        })
    };

    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(code.clone(), true, window, cx));
    assert_eq!(menu(cx), (None, Vec::new()), "only the text editor opens .rs files");

    workbench.update_in(cx, |workbench, window, cx| workbench.open_file(a.clone(), true, window, cx));
    let (label, actions) = menu(cx);
    assert_eq!(label.as_deref(), Some("Text Editor"));
    assert_eq!(actions, ["Text Editor *", "Markdown Editor", "-", "Set Default for '*.txt' [Text Editor *, Markdown Editor]"]);

    let uri = |path: &Path| json!({ "scheme": "file", "path": path.to_string_lossy() });
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("selectForCompare", vec![uri(&a)], window, cx)));
    workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("compareFiles", vec![uri(&b)], window, cx)));
    cx.run_until_parked();
    let (label, actions) = menu(cx);
    assert_eq!(label.as_deref(), Some("Text Diff Editor"));
    assert_eq!(actions, ["Text Diff Editor *", "Markdown Editor", "-", "Set Default (Diff Only) for '*.txt' [Text Diff Editor *, Markdown Editor]", "-", "Open Original", "Open Modified"]);
    std::fs::remove_dir_all(dir).unwrap();
}
