use std::path::Path;
use std::sync::{Arc, Mutex};

use config::registry::Registry;
use config::service::Settings;
use config::workspace::{Workspace, WorkspaceFolder};
use exthost::scan::scan_extension;
use gpui::TestAppContext;
use serde_json::json;

use crate::editor::{Position, Selection};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

#[gpui::test]
fn string_arguments_resolve_against_the_active_file_and_its_folder(cx: &mut TestAppContext) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let theme = load_theme(&[scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap()], "Dark 2026").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-resolver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project/src")).unwrap();
    std::fs::write(dir.join("project/src/main.ts"), "let answer = 42;\n").unwrap();
    std::fs::create_dir_all(dir.join("User")).unwrap();
    std::fs::write(dir.join("User/settings.json"), "{ \"editor.tabSize\": 2 }").unwrap();
    let folder = dir.join("project");
    let workspace = Workspace { file: None, folders: vec![WorkspaceFolder { path: folder.clone(), name: "project".into() }], transient: false };
    let settings = Arc::new(Mutex::new(Settings::load(Registry::core(), dir.join("User"), Some(workspace.clone()))));
    let (sender, _messages) = std::sync::mpsc::channel();
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(workspace), Languages::new(&[]), settings, HostSync::new(sender), cx));

    workbench.update_in(cx, |workbench, window, cx| {
        workbench.open_file(folder.join("src/main.ts"), true, window, cx);
        let editor = workbench.editors.active_mut().unwrap();
        editor.selections = vec![Selection { anchor: Position::new(0, 4), head: Position::new(0, 10) }];

        let args = json!({
            "query": "${selectedText} at ${lineNumber}:${columnNumber}",
            "filesToInclude": "${workspaceFolder}/src,${relativeFileDirname}",
            "filesToExclude": "${workspaceFolderBasename} ${config:editor.tabSize}",
            "isRegexp": true,
        });
        let resolved = workbench.resolve_string_arguments(args).unwrap();
        assert_eq!(resolved["query"], "answer at 1:11");
        assert_eq!(resolved["filesToInclude"], format!("{}/src,src", folder.display()));
        assert_eq!(resolved["filesToExclude"], "project 2");
        assert_eq!(resolved["isRegexp"], true);

        let error = workbench.resolve_string_arguments(json!({ "query": "${workspaceFolder:missing}" })).unwrap_err();
        assert_eq!(error.message, "Variable workspaceFolder can not be resolved. No such folder 'missing'.");
    });
    std::fs::remove_dir_all(dir).unwrap();
}
