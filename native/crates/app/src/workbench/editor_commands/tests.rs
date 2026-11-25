use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use gpui::TestAppContext;
use serde_json::json;

use crate::commands::Outcome;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

// textMateTokenizationFeature.contribution.ts `_workbench.colorizeTextMateTokens`, as vscode-colorize-perf-tests calls it.
#[gpui::test]
fn colorize_text_mate_tokens_times_a_file_and_fails_without_a_tokenizer(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let extensions = exthost::scan::scan_builtin_extensions(&app_root.join("extensions")).unwrap();
    let theme = load_theme(&extensions, "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-colorize-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ts"), "let a = 1;\nfunction f() {}\n").unwrap();
    std::fs::write(dir.join("a.txt"), "text").unwrap();
    let settings = Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), dir.join("User"), None)));
    let (sender, _messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&extensions), settings, HostSync::new(sender), cx));
    let mut colorize = |file: &str| {
        let uri = json!({ "scheme": "file", "path": dir.join(file) });
        match workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_workbench.colorizeTextMateTokens", vec![uri], window, cx)) {
            Outcome::Ready(result) => result,
            Outcome::Pending(_) => panic!("answers at once"),
        }
    };
    assert!(colorize("a.ts").unwrap()["tokenizeTime"].as_f64().unwrap() >= 0.);
    assert_eq!(colorize("a.txt").unwrap_err()["message"], "Cannot resolve tokenizer for language plaintext");
    let missing = colorize("missing.ts").unwrap_err();
    assert!(missing["message"].as_str().unwrap().starts_with("Cannot resolve text model for resource file:///"), "{missing}");
    std::fs::remove_dir_all(dir).unwrap();
}
