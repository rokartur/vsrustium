use std::path::Path;

use config::workspace::Workspace;
use exthost::scan::scan_extension;
use exthost::session::ForwardedRequest;
use gpui::TestAppContext;
use serde_json::{Value, json};

use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn explorer_rows_ask_each_provider_once_and_show_the_answer(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-decorations-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.ts"), "").unwrap();
    let dir = dir.canonicalize().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let folder = Workspace::folder(dir.clone());
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root, Some(folder), Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx));
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| drop(window.draw(cx)));
        cx.run_until_parked();
    };
    drop(workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("workbench.view.explorer", Vec::new(), window, cx)));
    draw(cx);
    receiver.try_iter().for_each(drop);

    let register = ForwardedRequest { req: 1, proxy: "MainThreadDecorations", method: "$registerDecorationProvider".into(), args: vec![json!(7), json!("Git")] };
    drop(workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(register, window, cx)));
    draw(cx);
    let mut asked = Vec::new();
    for message in receiver.try_iter().filter(|message| message.method == "$provideDecorations") {
        assert_eq!(message.args[0], json!(7));
        let mut reply = serde_json::Map::new();
        for request in message.args[1].as_array().unwrap() {
            let path = request["uri"]["path"].as_str().unwrap().to_owned();
            if path.ends_with("src") {
                reply.insert(request["id"].to_string(), json!([false, "Folder", "F", null]));
            }
            asked.push(path);
        }
        let _ = message.reply.unwrap().send(Ok(Value::Object(reply)));
    }
    draw(cx);
    assert_eq!(asked, vec![dir.join("src").to_string_lossy().into_owned()], "the one visible row");
    draw(cx);
    assert_eq!(receiver.try_iter().filter(|message| message.method == "$provideDecorations").count(), 0, "cached");
    let letter = workbench.read_with(cx, |workbench, _| workbench.file_decoration(&dir.join("src"), true).and_then(|decoration| decoration.letter));
    assert_eq!(letter.as_deref(), Some("F"));
}

#[gpui::test]
fn problems_decorate_the_file_before_other_providers_and_bubble_to_its_folder(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark Modern").unwrap();
    let dir = Path::new("/w");
    let (sender, _receiver) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root, None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx));
    let marker = |severity: u64| json!({ "severity": severity, "message": "m", "startLineNumber": 1, "startColumn": 1, "endLineNumber": 1, "endColumn": 2 });
    let uri = json!({ "$mid": 1, "scheme": "file", "path": "/w/src/a.ts" });
    workbench.update(cx, |workbench, cx| {
        let mut decorations = workbench.decorations.borrow_mut();
        decorations.register(7);
        decorations.set(7, "/w/src/a.ts", crate::decorations::DecorationData::from_reply(&json!([false, "Modified", "M", { "id": "gitDecoration.modifiedResourceForeground" }])));
        drop(decorations);
        workbench.change_many_markers("ts", &json!([[uri, [marker(4), marker(8)]]]), cx);
    });
    let file = workbench.read_with(cx, |workbench, _| workbench.file_decoration(&dir.join("src/a.ts"), false)).unwrap();
    assert_eq!((file.letter.as_deref(), file.color.as_deref(), file.tooltip.as_deref()), (Some("2, M"), Some("list.errorForeground"), Some("2 problems in this file \u{2022} Modified")));
    let folder = workbench.read_with(cx, |workbench, _| workbench.file_decoration(&dir.join("src"), true)).unwrap();
    assert!(folder.bubbled);

    workbench.update(cx, |workbench, cx| workbench.change_many_markers("ts", &json!([[uri, [marker(1)]]]), cx));
    let file = workbench.read_with(cx, |workbench, _| workbench.file_decoration(&dir.join("src/a.ts"), false)).unwrap();
    assert_eq!(file.letter.as_deref(), Some("M"), "hints and infos do not decorate");
}
