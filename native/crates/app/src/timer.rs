//! workbench/services/timer timerService.ts `_computeStartupMetrics` and perfviewEditor.ts `_addSummaryTable`: the
//! startup phases between the marks main.rs sets. The port is one native process, so the phases of Electron's main
//! process bundle, crash reporter, IPC server, renderer bundle, window config and shared process have no marks and
//! no rows; every window is an initial startup of a new window.

/// One row per measured phase: what, duration in ms, process, info; phases without both marks are left out.
pub fn summary_rows(between: impl Fn(&str, &str) -> Option<f64>, editor_ids: &[String]) -> Vec<Vec<String>> {
    let initial = "initial startup: true".to_owned();
    let editors = format!("{}: {}", editor_ids.len(), editor_ids.join(", "));
    let phases = [
        ("start => app.isReady", "code/didStartMain", "code/mainAppReady", "[main]", initial.clone()),
        ("nls:start => nls:end", "code/willGenerateNls", "code/didGenerateNls", "[main]", initial.clone()),
        ("create window", "code/willCreateCodeWindow", "code/didCreateCodeWindow", "[main]", initial.clone()),
        ("app.isReady => window.loadUrl()", "code/mainAppReady", "code/willOpenNewWindow", "[main]", initial),
        ("init storage (global & workspace)", "code/willInitStorage", "code/didInitStorage", "[renderer]", "-".to_owned()),
        ("init workspace service", "code/willInitWorkspaceService", "code/didInitWorkspaceService", "[renderer]", "-".to_owned()),
        ("register extensions & spawn extension host", "code/willLoadExtensions", "code/didLoadExtensions", "[renderer]", "-".to_owned()),
        ("restore & resolve visible editors", "code/willRestoreEditors", "code/didRestoreEditors", "[renderer]", editors),
        ("create workbench contributions", "code/willCreateWorkbenchContributions/1", "code/didCreateWorkbenchContributions/2", "[renderer]", "-".to_owned()),
        ("overall workbench load", "code/willStartWorkbench", "code/didStartWorkbench", "[renderer]", "-".to_owned()),
        ("workbench ready", "code/didStartMain", "code/didStartWorkbench", "[main->renderer]", "-".to_owned()),
        ("extensions registered", "code/didStartMain", "code/didLoadExtensions", "[renderer]", "-".to_owned()),
    ];
    let mut rows = Vec::new();
    for (what, from, to, process, info) in phases {
        if let Some(duration) = between(from, to) {
            rows.push(vec![what.to_owned(), duration.to_string(), process.to_owned(), info]);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::summary_rows;

    #[test]
    fn rows_need_both_marks_and_keep_the_upstream_order() {
        let marks = [("code/didStartMain", 0.), ("code/mainAppReady", 12.), ("code/willRestoreEditors", 40.), ("code/didRestoreEditors", 45.), ("code/didStartWorkbench", 90.)];
        let time = |name: &str| marks.iter().find(|(mark, _)| *mark == name).map(|(_, time)| *time);
        let rows = summary_rows(|from, to| Some(time(to)? - time(from)?), &["workbench.editors.files.textFileEditor".into()]);
        let whats: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
        assert_eq!(whats, ["start => app.isReady", "restore & resolve visible editors", "workbench ready"]);
        assert_eq!(rows[1], ["restore & resolve visible editors", "5", "[renderer]", "1: workbench.editors.files.textFileEditor"]);
        assert_eq!(rows[2][1], "90");
    }
}
