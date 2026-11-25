//! perfviewEditor.ts: Developer: Startup Performance opens the read-only markdown model `perf:Startup Performance`
//! with the system, the port's startup marks, the extension activation times and the raw marks of each source.

use std::path::PathBuf;
use std::process::Command;

use gpui::{Context, Window};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::TextDocument;

pub const PERF_SCHEME: &str = "perf";
const PERF_PATH: &str = "Startup Performance";

impl Workbench {
    pub(super) fn register_perf_view(&mut self) {
        self.commands.register("perfview.show", |workbench, _, window, cx| workbench.show_perf_view(window, cx));
    }

    /// timerService.ts `editorIds`: the ids of the visible editor panes.
    pub(crate) fn visible_editor_ids(&self) -> Vec<String> {
        let pane = |scheme: &str| match scheme {
            "file" => "workbench.editors.files.textFileEditor".to_owned(),
            "untitled" => "workbench.editors.textResourceEditor".to_owned(),
            super::welcome_page::WALKTHROUGH_SCHEME => "gettingStartedPage".to_owned(),
            scheme => scheme.to_owned(),
        };
        self.editors.active().map(|editor| pane(&editor.document.scheme)).into_iter().collect()
    }

    /// Opens the model pinned, or refreshes it in its open editor, as `PerfModelContentProvider._updateModel` does.
    fn show_perf_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let text = self.perf_markdown(window);
        let path = PathBuf::from(PERF_PATH);
        match self.editors.models.iter().position(|model| model.document.is(PERF_SCHEME, &path)) {
            Some(index) => {
                self.replace_model_text(PERF_SCHEME, &path, text, true, cx);
                self.editors.show_model(index, true);
            }
            None => {
                let document = TextDocument::from_text(PERF_SCHEME, &path, &text, "utf8".into(), Some("markdown"), &self.languages, &self.settings.lock().unwrap());
                let index = self.editors.open_model(document);
                self.editors.show_model(index, true);
            }
        }
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
        Outcome::done()
    }

    fn perf_markdown(&self, window: &Window) -> String {
        let mut md = String::new();
        heading(&mut md, "System Info");
        for line in system_info(window.is_a11y_active(), self.workspace().is_none()) {
            md += &format!("* {line}\n");
        }
        md += "\n";
        heading(&mut md, "Performance Marks");
        let rows = crate::timer::summary_rows(crate::perf::between, &crate::perf::startup_editors());
        md += &table(&["What", "Duration", "Process", "Info"], &rows);
        md += "\n";
        let activations = crate::perf::activations();
        if !activations.is_empty() {
            heading(&mut md, "Extension Activation Stats");
            md += &table(&["Extension", "Eager", "Load Code", "Call Activate", "Finish Activate", "Event", "By"], &activations);
        }
        md += "\n";
        for (source, marks) in crate::perf::performance_marks() {
            heading(&mut md, &format!("Raw Perf Marks: {source}"));
            md += "```\nName\tTimestamp\tDelta\tTotal\n";
            let (mut last, mut total) = (None, 0.);
            for (name, time) in marks {
                let delta = last.map_or(0., |last| time - last);
                total += delta;
                md += &format!("{name}\t{time}\t{delta}\t{total}\n");
                last = Some(time);
            }
            md += "```\n";
        }
        md
    }
}

fn heading(md: &mut String, value: &str) {
    *md += &format!("## {value}\n\n");
}

/// `MarkdownBuilder.toMarkdownTable`: cells padded to their column's widest.
fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|cell| cell.chars().count()).collect();
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    let line = |cells: Vec<String>| cells.iter().zip(&widths).map(|(cell, width)| format!("| {cell}{} ", " ".repeat(width - cell.chars().count()))).collect::<String>() + "|\n";
    let mut text = line(header.iter().map(|cell| cell.to_string()).collect());
    text += &line(widths.iter().map(|width| "-".repeat(*width)).collect());
    for row in rows {
        text += &line(row.clone());
    }
    text
}

/// `_addSummary` from sysctl, vm_stat and ps, where upstream reads Node's `os` and Electron's process metrics.
fn system_info(screen_reader: bool, empty_workspace: bool) -> Vec<String> {
    let product = |key: &str| crate::product::get(key).as_str().unwrap_or_default().to_owned();
    let commit = Some(product("commit")).filter(|commit| !commit.is_empty()).unwrap_or("0000000".into());
    let sysctl = run("sysctl", &["-n", "kern.osrelease", "machdep.cpu.brand_string", "hw.ncpu", "hw.memsize"]);
    let sysctl: Vec<&str> = sysctl.lines().collect();
    let total_memory = sysctl.get(3).and_then(|bytes| bytes.parse::<f64>().ok()).unwrap_or_default();
    let resident_kb = run("ps", &["-o", "rss=", "-p", &std::process::id().to_string()]).trim().parse::<f64>().unwrap_or_default();
    let gb = 1024. * 1024. * 1024.;
    vec![
        format!("{}: {} ({commit})", product("nameShort"), product("version")),
        format!("OS: darwin({})", sysctl.first().unwrap_or(&"")),
        // shortcut: speed 0 and no private/shared memory or VM likelihood: libuv reads the speed from IOKit and Electron
        // the process metrics; add them if the view is used to compare machines.
        format!("CPUs: {}({} x 0)", sysctl.get(1).unwrap_or(&""), sysctl.get(2).unwrap_or(&"")),
        format!("Memory(System): {:.2} GB({:.2}GB free)", total_memory / gb, free_memory() / gb),
        format!("Memory(Process): {:.2} MB working set", resident_kb / 1024.),
        "Initial Startup: true".into(),
        "Has 0 other windows".into(),
        format!("Screen Reader Active: {screen_reader}"),
        format!("Empty Workspace: {empty_workspace}"),
    ]
}

/// `os.freemem()` on macOS: free pages times the page size (libuv's `uv_get_free_memory`).
fn free_memory() -> f64 {
    let vm_stat = run("vm_stat", &[]);
    let page_size = vm_stat.split("page size of ").nth(1).and_then(|rest| rest.split(' ').next()).and_then(|size| size.parse::<f64>().ok()).unwrap_or(16384.);
    let free_pages = vm_stat.lines().find(|line| line.starts_with("Pages free:")).and_then(|line| line.split(':').nth(1)).and_then(|pages| pages.trim().trim_end_matches('.').parse::<f64>().ok()).unwrap_or_default();
    free_pages * page_size
}

fn run(program: &str, args: &[&str]) -> String {
    Command::new(program).args(args).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::table;

    #[test]
    fn the_table_pads_each_column_to_its_widest_cell() {
        let rows = vec![vec!["vscode.git".to_owned(), "12".to_owned()]];
        assert_eq!(table(&["Extension", "Load Code"], &rows), "| Extension  | Load Code |\n| ---------- | --------- |\n| vscode.git | 12        |\n");
    }
}
