use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use exthost::scan::scan_extension;
use exthost::session::ForwardedRequest;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::host_sync::{HostMessage, HostSync};
use crate::languages::Languages;
use crate::theme::load_theme;
use crate::workbench::Workbench;

fn workbench_with<'a>(cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext, Receiver<HostMessage>, std::path::PathBuf) {
    let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("vsrustium-folding-providers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "import a\nimport b\nimport c\nx\n  y\n  z").unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let file = dir.join("a.txt");
    let (workbench, cx) = cx.add_window_view(|window, cx| {
        let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
        workbench.open_file(file, true, window, cx);
        workbench
    });
    (workbench, cx, receiver, dir)
}

fn request(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, method: &str, args: Value) {
    let request = ForwardedRequest { req: 1, proxy: "MainThreadLanguageFeatures", method: method.into(), args: args.as_array().unwrap().clone() };
    drop(workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx)));
}

/// Waits out the update delay and answers the `$provideFoldingRanges` calls it sends, by provider handle.
fn answer(receiver: &Receiver<HostMessage>, cx: &mut VisualTestContext, answers: &[(u64, Value)]) -> Vec<u64> {
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let mut asked = Vec::new();
    for message in receiver.try_iter() {
        if message.method != "$provideFoldingRanges" {
            continue;
        }
        assert!(message.cancellable, "the host gets a CancellationToken");
        let handle = message.args[0].as_u64().unwrap();
        asked.push(handle);
        let answer = answers.iter().find(|(provider, _)| *provider == handle).map_or(Value::Null, |(_, answer)| answer.clone());
        let _ = message.reply.unwrap().send(Ok(answer));
    }
    cx.run_until_parked();
    asked
}

fn regions(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(usize, usize, bool)> {
    workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().folding.regions.ranges().iter().map(|range| (range.start, range.end, range.collapsed)).collect())
}

#[gpui::test]
fn extension_providers_fold_the_model_and_indentation_is_the_fallback(cx: &mut TestAppContext) {
    let (workbench, cx, receiver, dir) = workbench_with(cx);
    assert_eq!(regions(&workbench, cx), [(4, 6, false)], "indentation folds first");
    request(&workbench, cx, "$registerFoldingRangeProvider", json!([1, [{ "language": "plaintext" }], { "value": "ext.one" }, 7]));
    request(&workbench, cx, "$registerFoldingRangeProvider", json!([2, [{ "language": "markdown" }], { "value": "ext.two" }, null]));
    let ranges = json!([{ "start": 1, "end": 3, "kind": { "value": "imports" } }, { "start": 2, "end": 9 }]);
    assert_eq!(answer(&receiver, cx, &[(1, ranges.clone())]), [1], "only the provider whose selector matches");
    assert_eq!(regions(&workbench, cx), [(1, 3, false)], "a range past the last line is dropped");

    // `$emitFoldingRangeEvent` with the provider's event handle asks again.
    request(&workbench, cx, "$emitFoldingRangeEvent", json!([7]));
    assert_eq!(answer(&receiver, cx, &[(1, json!([{ "start": 4, "end": 6 }]))]), [1]);
    assert_eq!(regions(&workbench, cx), [(4, 6, false)]);

    // An answer without an array falls back to the indentation ranges.
    request(&workbench, cx, "$emitFoldingRangeEvent", json!([7]));
    assert_eq!(answer(&receiver, cx, &[]), [1]);
    assert!(!workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().syntax_folding));
    request(&workbench, cx, "$unregister", json!([1]));
    assert_eq!(answer(&receiver, cx, &[]), Vec::<u64>::new());
    assert_eq!(regions(&workbench, cx), [(4, 6, false)]);
    std::fs::remove_dir_all(dir).unwrap();
}
