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
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
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
