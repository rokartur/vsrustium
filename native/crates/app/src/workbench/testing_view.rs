//! The Testing view container (testingExplorerView.ts), the Test Results editor (testResultsViewContent.ts with
//! its tree and the task output), the run glyphs of testingDecorations.ts and the commands of
//! testExplorerActions.ts.

use std::collections::HashSet;

use gpui::{ClickEvent, Context, Div, MouseButton, MouseDownEvent, Rgba, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use super::panel::PanelView;
use crate::editor::OpenEditor;
use crate::testing::{ExpandState, GROUP_DEBUG, GROUP_RUN, ResultState};

/// testing.contribution.ts `Testing.ResultsPanelId`, the panel container of the Test Results view.
pub const TEST_RESULTS_VIEW_ID: &str = "workbench.panel.testResults";
const ROW_HEIGHT: f32 = 22.;

/// The view state of the explorer and the results editor.
#[derive(Default)]
pub struct TestingView {
    /// Rows the user collapsed; others below the roots start collapsed until expanded (`expanded`).
    pub collapsed: HashSet<String>,
    pub expanded: HashSet<String>,
    /// The result and test the results editor shows the output of.
    pub selected_result: Option<String>,
    pub selected_test: Option<String>,
}

/// testingStates icons (icons.ts `testingStatesToIcons`) and their colors (theme.ts `testStatesToIconColors`).
pub fn state_icon(state: ResultState) -> (&'static str, &'static str) {
    match state {
        ResultState::Errored => ("issues", "testing.iconErrored"),
        ResultState::Failed => ("error", "testing.iconFailed"),
        ResultState::Passed => ("pass", "testing.iconPassed"),
        ResultState::Queued => ("history", "testing.iconQueued"),
        ResultState::Running => ("loading", "icon.foreground"),
        ResultState::Skipped => ("debug-step-over", "testing.iconSkipped"),
        ResultState::Unset => ("circle-outline", "testing.iconUnset"),
    }
}

impl Workbench {
    pub(super) fn register_testing_commands(&mut self) {
        self.commands.register("workbench.view.testing", |workbench, _, _, cx| {
            workbench.side_bar_view = super::search_view::SideBarView::Testing;
            workbench.set_part_hidden(super::layout::Part::SideBar, false, cx);
            Outcome::done()
        });
        self.commands.register("workbench.view.extension.test", |workbench, args, window, cx| workbench.execute_command("workbench.view.testing", args, window, cx));
        for (id, group) in [("testing.runAll", GROUP_RUN), ("testing.debugAll", GROUP_DEBUG)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let roots = workbench.testing.collection.roots.clone();
                workbench.run_or_notify(&roots, group, cx);
                Outcome::done()
            });
        }
        for (id, group) in [("testing.runCurrentFile", GROUP_RUN), ("testing.debugCurrentFile", GROUP_DEBUG)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let Some(path) = workbench.editors.active().map(|editor| editor.document().path.to_string_lossy().into_owned()) else { return Outcome::done() };
                let tests = workbench.top_tests_in_file(&path);
                workbench.run_or_notify(&tests, group, cx);
                Outcome::done()
            });
        }
        for (id, group) in [("testing.runAtCursor", GROUP_RUN), ("testing.debugAtCursor", GROUP_DEBUG)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let Some(editor) = workbench.editors.active() else { return Outcome::done() };
                let line = editor.selections[0].head.line as u32 + 1;
                let tests = workbench.tests_at_line(&editor.document().path.to_string_lossy(), line);
                workbench.run_or_notify(&tests, group, cx);
                Outcome::done()
            });
        }
        for (id, group) in [("testing.runTest", GROUP_RUN), ("testing.debugTest", GROUP_DEBUG)] {
            self.commands.register(id, move |workbench, args, _, cx| {
                let ids: Vec<String> = args.iter().filter_map(|arg| arg.as_str().or(arg["id"].as_str()).map(str::to_owned)).collect();
                workbench.run_or_notify(&ids, group, cx);
                Outcome::done()
            });
        }
        // `vscode.runTestsById`-style entry testing.contribution.ts registers for `runTests` of a test id list.
        self.commands.register("vscode.runTestsById", |workbench, args, _, cx| {
            let group = args.first().and_then(Value::as_u64).unwrap_or(GROUP_RUN);
            let ids: Vec<String> = args.iter().skip(1).filter_map(|arg| arg.as_str().map(str::to_owned)).collect();
            Outcome::Ready(Ok(json!(workbench.run_tests(&ids, group, cx))))
        });
        self.commands.register("testing.reRunLastRun", |workbench, _, _, cx| {
            let Some(last) = workbench.testing.results.first() else { return Outcome::done() };
            let ids: Vec<String> = last.request["targets"].as_array().into_iter().flatten().flat_map(|target| target["testIds"].as_array().into_iter().flatten()).filter_map(|id| id.as_str().map(str::to_owned)).collect();
            let group = last.request["group"].as_u64().unwrap_or(GROUP_RUN);
            workbench.run_or_notify(&ids, group, cx);
            Outcome::done()
        });
        self.commands.register("testing.cancelRun", |workbench, _, _, _| {
            workbench.cancel_test_runs();
            Outcome::done()
        });
        self.commands.register("testing.refreshTests", |workbench, _, _, _| {
            let controllers: Vec<String> = workbench.testing.controllers.iter().filter(|(_, controller)| controller.capabilities & 2 != 0).map(|(id, _)| id.clone()).collect();
            for controller in controllers {
                workbench.host.send("ExtHostTesting", "$refreshTests", vec![json!(controller)]);
            }
            Outcome::done()
        });
        self.commands.register("testing.clearTestResults", |workbench, _, _, cx| {
            workbench.testing.results.retain(|result| result.completed_at.is_none());
            cx.notify();
            Outcome::done()
        });
        self.commands.register("testing.showMostRecentOutput", |workbench, _, _, cx| {
            workbench.open_panel_view(TEST_RESULTS_VIEW_ID, cx);
            Outcome::done()
        });
        self.commands.register("testing.searchForTestExtension", |workbench, _, window, cx| workbench.execute_command("workbench.extensions.search", vec![json!("@category:\"testing\"")], window, cx));
        self.commands.register("testing.collapseAll", |workbench, _, _, cx| {
            workbench.testing_view.expanded.clear();
            cx.notify();
            Outcome::done()
        });
    }

    /// The command's tests, else testExplorerActions.ts's "No tests found here" notification.
    fn run_or_notify(&mut self, ids: &[String], group: u64, cx: &mut Context<Self>) {
        if self.run_tests(ids, group, cx).is_none() {
            self.notify_error("No tests found here");
        }
        cx.notify();
    }

    /// The tests of the file that have no ancestor in it: what Run Tests in Current File runs.
    fn top_tests_in_file(&self, path: &str) -> Vec<String> {
        let collection = &self.testing.collection;
        let in_file: HashSet<&str> = collection.items.values().filter(|item| item.path() == Some(path)).map(|item| item.id.as_str()).collect();
        let mut ids: Vec<String> = in_file.iter().filter(|id| crate::testing::parent_id(id).is_none_or(|parent| !in_file.contains(parent))).map(|id| id.to_string()).collect();
        ids.sort();
        ids
    }

    /// testingDecorations.ts: the tests whose range starts on the line; the innermost at the cursor runs.
    fn tests_at_line(&self, path: &str, line: u32) -> Vec<String> {
        let mut ids: Vec<String> = self.testing.collection.tests_in_file(path).filter(|item| item.start_line() == Some(line)).map(|item| item.id.clone()).collect();
        ids.sort();
        ids
    }

    /// `testing.automaticallyOpenTestResults`: `openOnTestStart` opens the results without focus.
    pub(super) fn show_test_results_on_start(&mut self, run: &str, cx: &mut Context<Self>) {
        self.testing_view.selected_result = Some(run.to_owned());
        self.testing_view.selected_test = None;
        if self.setting("testing.automaticallyOpenTestResults") == "openOnTestStart" {
            self.open_panel_view(TEST_RESULTS_VIEW_ID, cx);
        }
    }

    pub(super) fn test_output_changed(&mut self, _run: &str) {}

    /// `openOnTestFailure` opens the results when a run ends with a failure.
    pub(super) fn test_run_finished(&mut self, run: &str, cx: &mut Context<Self>) {
        let failed = self.testing.results.iter().find(|result| result.id == run).is_some_and(|result| result.count(ResultState::Failed) + result.count(ResultState::Errored) > 0);
        if failed && self.setting("testing.automaticallyOpenTestResults") == "openOnTestFailure" {
            self.testing_view.selected_result = Some(run.to_owned());
            self.open_panel_view(TEST_RESULTS_VIEW_ID, cx);
        }
    }

    /// testing.contribution.ts registers the Test Results container in the panel at order 3, `hideIfEmpty`: its
    /// view's `when` is `testing.hasAnyResults`.
    pub(super) fn register_test_results_panel_view(&mut self) {
        self.register_panel_view(PanelView {
            id: TEST_RESULTS_VIEW_ID,
            title: "Test Results",
            render: |workbench, _, cx| workbench.test_results_body(cx).into_any_element(),
            actions: |_, _, _| gpui::Empty.into_any_element(),
            visible: |workbench| !workbench.testing.results.is_empty(),
        });
    }

    /// The Testing side bar: the welcome without controllers, else the tests tree with the view's actions.
    pub(super) fn testing_side_bar(&self, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBar.foreground");
        let running = self.testing.is_running();
        let has_tests = !self.testing.collection.roots.is_empty();
        let mut actions = div().flex().gap(px(2.)).mr(px(4.));
        if has_tests {
            actions = actions.child(self.search_action("refresh", "testing.refreshTests", self.testing.controllers.values().any(|controller| controller.capabilities & 2 != 0), cx));
            actions = actions.child(match running {
                true => self.search_action("debug-stop", "testing.cancelRun", true, cx),
                false => self.search_action("run-all", "testing.runAll", true, cx),
            });
            actions = actions.child(self.search_action("debug-alt-small", "testing.debugAll", !running, cx));
            actions = actions.child(self.search_action("collapse-all", "testing.collapseAll", true, cx));
        }
        let title = self.side_bar_title("Testing").child(actions);
        let mut body = div().id("testing-tree").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if !has_tests {
            // testingExplorerView.ts `testingViewWelcome`, for a workspace without a test provider.
            body = body.child(div().px(px(20.)).py(px(13.)).text_size(px(13.)).child("No tests have been found in this workspace yet."));
            body = body.child(div().px(px(20.)).child(self.testing_button("Install Additional Test Extensions...", "testing.searchForTestExtension", cx)));
        } else {
            let mut rows = Vec::new();
            let roots = self.testing.collection.roots.clone();
            // index.ts: a single root shows its children at the top level.
            match roots.as_slice() {
                [only] => {
                    for child in self.testing.collection.children_of(only) {
                        self.test_rows(child, 0, &mut rows, cx);
                    }
                }
                _ => {
                    for root in &roots {
                        self.test_rows(root, 0, &mut rows, cx);
                    }
                }
            }
            body = body.children(rows);
        }
        self.side_bar_frame().text_color(foreground).child(title).child(body)
    }

    fn testing_button(&self, label: &str, command: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        div()
            .id(command)
            .h(px(26.))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(self.color("button.background"))
            .text_color(self.color("button.foreground"))
            .cursor_pointer()
            .child(label.to_owned())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                workbench.execute_command(command, Vec::new(), window, cx);
            }))
    }

    fn test_rows(&self, id: &str, depth: usize, rows: &mut Vec<gpui::AnyElement>, cx: &mut Context<Self>) {
        let Some(item) = self.testing.collection.items.get(id) else { return };
        let children = self.testing.collection.children_of(id);
        let expandable = item.expand != ExpandState::NotExpandable || !children.is_empty();
        // index.ts: the top level starts expanded, deeper rows collapsed.
        let expanded = expandable && !self.testing_view.collapsed.contains(id) && (depth == 0 || self.testing_view.expanded.contains(id));
        let state = self.testing.state_of(id).unwrap_or_default();
        let (icon, color) = state_icon(state);
        let icon_color = self.foreground_or(color);
        let twistie_color = self.foreground_or("icon.foreground");
        let hover = self.color("list.hoverBackground");
        let (row_id, toggle_id, expand) = (id.to_owned(), id.to_owned(), item.expand);
        let run_id = id.to_owned();
        let debug_id = id.to_owned();
        let has_debug = self.testing.profiles.iter().any(|profile| profile.group & GROUP_DEBUG != 0 && profile.controller_id == item.controller_id());
        let actions = div()
            .ml_auto()
            .flex()
            .invisible()
            .group_hover("testing-row", |actions| actions.visible())
            .child(self.icon("run", 22., 16., self.foreground_or("icon.foreground")).id(gpui::SharedString::from(format!("run:{id}"))).cursor_pointer().on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                workbench.run_or_notify(std::slice::from_ref(&run_id), GROUP_RUN, cx);
            })))
            .when(has_debug, |actions| {
                actions.child(self.icon("debug-alt-small", 22., 16., self.foreground_or("icon.foreground")).id(gpui::SharedString::from(format!("debug:{id}"))).cursor_pointer().on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    workbench.run_or_notify(std::slice::from_ref(&debug_id), GROUP_DEBUG, cx);
                })))
            });
        let mut row = div()
            .id(gpui::SharedString::from(format!("test:{id}")))
            .group("testing-row")
            .h(px(ROW_HEIGHT))
            .mx(px(4.))
            .pl(px(8. * depth as f32 + 4.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .child(match expandable {
                true => self.icon(if expanded { "chevron-down" } else { "chevron-right" }, 16., 16., twistie_color),
                false => div().w(px(16.)).flex_none(),
            })
            .child(self.icon(icon, 22., 16., icon_color))
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(item.label().to_owned()));
        if let Some(description) = item.item["description"].as_str() {
            row = row.child(div().ml(px(6.)).text_size(px(12.)).opacity(0.7).whitespace_nowrap().child(description.to_owned()));
        }
        row = row.child(actions).on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
            if event.click_count() == 2 {
                workbench.reveal_test(&row_id, window, cx);
                return;
            }
            if !expandable {
                workbench.reveal_test(&row_id, window, cx);
                return;
            }
            let view = &mut workbench.testing_view;
            match expanded {
                true => {
                    view.expanded.remove(&toggle_id);
                    view.collapsed.insert(toggle_id.clone());
                }
                false => {
                    view.collapsed.remove(&toggle_id);
                    view.expanded.insert(toggle_id.clone());
                    // testingExplorerView.ts `onDidChangeCollapseState`: expanding resolves the children.
                    if expand == ExpandState::Expandable {
                        workbench.host.send("ExtHostTesting", "$expandTest", vec![json!(toggle_id), json!(0)]);
                    }
                }
            }
            cx.notify();
        }));
        rows.push(row.into_any_element());
        if expanded {
            for child in children {
                self.test_rows(child, depth + 1, rows, cx);
            }
        }
    }

    /// testExplorerActions.ts `GoToTest`: the test's file at its range.
    fn reveal_test(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.testing.collection.items.get(id) else { return };
        let (Some(path), Some(line)) = (item.path().map(std::path::PathBuf::from), item.start_line()) else { return };
        let column = item.item["range"]["startColumn"].as_u64().unwrap_or(1) as usize;
        self.open_file(path.clone(), true, window, cx);
        let line = line as usize;
        self.select_range(&path, crate::anything_quick_access::TextRange { start_line_number: line, start_column: column, end_line_number: line, end_column: column }, cx);
    }

    /// testingDecorations.ts `RunTestDecoration` glyph of the line: the tests starting there, the worst state of
    /// them, a run icon while none ran. A click runs them with `testing.defaultGutterClickAction`.
    pub(super) fn test_glyph(&self, editor: &OpenEditor, line: u32, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.setting("testing.gutterEnabled") == false || editor.document().scheme != "file" {
            return None;
        }
        let path = editor.document().path.to_string_lossy().into_owned();
        let tests = self.tests_at_line(&path, line);
        if tests.is_empty() {
            return None;
        }
        let state = tests.iter().fold(ResultState::Unset, |state, id| state.max(self.testing.state_of(id).unwrap_or_default()));
        let multiple = tests.len() > 1 || tests.iter().any(|id| !self.testing.collection.children_of(id).is_empty());
        let (icon, color) = match state {
            ResultState::Unset => (if multiple { "run-all" } else { "run" }, "testing.runAction"),
            state => state_icon(state),
        };
        let color: Rgba = self.foreground_or(color);
        let group = match self.setting("testing.defaultGutterClickAction").as_str() {
            Some("debug") => GROUP_DEBUG,
            _ => GROUP_RUN,
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(self.icon(icon, 16., 16., color))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    workbench.run_or_notify(&tests, group, cx);
                }))
                .into_any_element(),
        )
    }

    /// The Test Results editor: the selected result's tests with their states, then the output of its tasks.
    pub(super) fn test_results_body(&self, cx: &mut Context<Self>) -> Div {
        let result = match &self.testing_view.selected_result {
            Some(id) => self.testing.results.iter().find(|result| result.id == *id),
            None => self.testing.results.first(),
        };
        let mut tree = div().id("test-results-tree").w(px(260.)).flex_none().overflow_y_scroll().border_r_1().border_color(self.color("panel.border")).flex().flex_col();
        let mut output = div().id("test-results-output").flex_1().min_w_0().overflow_y_scroll().p(px(8.)).font_family("Menlo").text_size(px(12.)).flex().flex_col();
        let Some(result) = result else {
            return div().flex_1().min_h_0().flex().child(div().p(px(12.)).child("No test results yet."));
        };
        let mut queue: Vec<(String, usize)> = result.roots.iter().rev().map(|id| (id.clone(), 0)).collect();
        while let Some((id, depth)) = queue.pop() {
            let item = &result.items[&id];
            let state = result.computed_state(&id);
            let (icon, color) = state_icon(state);
            let duration = item.own_duration.map(|duration| format!("{}ms", duration.round())).unwrap_or_default();
            let selected = self.testing_view.selected_test.as_deref() == Some(id.as_str());
            let select_id = id.clone();
            tree = tree.child(
                div()
                    .id(gpui::SharedString::from(format!("result:{id}")))
                    .h(px(ROW_HEIGHT))
                    .pl(px(8. * depth as f32 + 4.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
                    .child(self.icon(icon, 22., 16., self.foreground_or(color)))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(item.label().to_owned()))
                    .child(div().ml_auto().mr(px(6.)).opacity(0.7).text_size(px(11.)).child(duration))
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                        workbench.testing_view.selected_test = Some(select_id.clone());
                        cx.notify();
                    })),
            );
            queue.extend(item.children.iter().rev().map(|child| (child.clone(), depth + 1)));
        }
        let lines: Vec<String> = match self.testing_view.selected_test.as_ref().and_then(|id| result.items.get(id)) {
            // The selected test's messages: its errors, then what it wrote.
            Some(item) => item.tasks.iter().flat_map(|task| task.messages.iter()).map(|message| message["message"].as_str().or(message["message"]["value"].as_str()).unwrap_or_default().to_owned()).collect(),
            None => result.tasks.iter().map(|task| task.output.clone()).collect(),
        };
        for line in lines.iter().flat_map(|text| super::debug_console::strip_ansi(text).lines().map(str::to_owned).collect::<Vec<_>>()) {
            output = output.child(div().whitespace_nowrap().child(line));
        }
        div().flex_1().min_h_0().flex().child(tree).child(output)
    }
}
