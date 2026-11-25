//! The merge editor (view/mergeEditor.ts, view/editors/inputCodeEditorView.ts, resultCodeEditorView.ts,
//! conflictActions.ts and commands/commands.ts): input 1 and input 2 read-only side by side on top, the result model
//! in the group's code editor below, each under a title bar; an action row above each modified base range.
// shortcut: no base view, no alignment zones and no gutter checkboxes; the inputs follow the result's scroll line by
// line through the range mappings. Add them when the merge editor gets its own code editors.

use std::path::Path;

use diff::LineRange;
use gpui::{Context, Div, Rgba, ScrollWheelEvent, StyledText, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::diff_editor_view::char_ranges;
use super::{EditorMetrics, Workbench};
use crate::commands::Outcome;
use crate::diff_editor::{Side, ZoneSide};
use crate::editor::{EditReason, OpenEditor, Position, TextDocument, VerticalReveal, expand_tabs};
use crate::merge_editor::{Input, InputData, LineEdit, MERGE_EDITOR_SCHEME, MergeInput, MergeModel, State, text_edit};
use crate::workbench::host_requests::uri_parts;

/// mergeEditor.css `.code-view > .title` height.
const TITLE_HEIGHT: f32 = 24.;

/// One action of a range's row: its label, and what it does when clicked (`None`: a label only).
type Action = (String, Option<MergeAction>);

#[derive(Clone, Copy)]
enum MergeAction {
    SetState(usize, State),
    Ignore(usize, Input),
}

impl Workbench {
    pub(in crate::workbench) fn register_merge_editor_commands(&mut self) {
        self.commands.register("_open.mergeEditor", |workbench, args, window, cx| workbench.open_merge_editor(args, window, cx));
        self.commands.register("merge.goToNextUnhandledConflict", |workbench, _, _, cx| {
            workbench.go_to_unhandled_conflict(true, cx);
            Outcome::done()
        });
        self.commands.register("merge.goToPreviousUnhandledConflict", |workbench, _, _, cx| {
            workbench.go_to_unhandled_conflict(false, cx);
            Outcome::done()
        });
        for (id, input) in [("merge.acceptAllInput1", Input::One), ("merge.acceptAllInput2", Input::Two)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                workbench.accept_all(input, cx);
                Outcome::done()
            });
        }
        for (id, input) in [("merge.toggleActiveConflictInput1", Input::One), ("merge.toggleActiveConflictInput2", Input::Two)] {
            self.commands.register(id, move |workbench, _, _, cx| workbench.toggle_active_conflict(input, cx));
        }
        self.commands.register("mergeEditor.resetResultToBaseAndAutoMerge", |workbench, _, _, cx| {
            workbench.reset_merge_result(cx);
            Outcome::done()
        });
        self.commands.register("mergeEditor.acceptMerge", |workbench, _, window, cx| workbench.accept_merge(window, cx));
    }

    /// commands.ts `OpenMergeEditor`: `{ base, input1: { uri, title, description, detail }, input2, output }`. The
    /// models open first, a missing one from its content or file system provider.
    fn open_merge_editor(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = args.first().cloned().unwrap_or(Value::Null);
        let uris = [arg["base"].clone(), arg["input1"]["uri"].clone(), arg["input2"]["uri"].clone(), arg["output"].clone()];
        let mut sides = Vec::new();
        for uri in &uris {
            match uri_parts(uri) {
                Ok((scheme, path)) => sides.push(Side { scheme: scheme.to_string(), path }),
                Err(error) => return Outcome::Ready(Err(error)),
            }
        }
        let missing: Vec<Value> = uris.iter().zip(&sides).filter(|(_, side)| side.scheme != "file" && self.editors.model(&side.scheme, &side.path).is_none()).map(|(uri, _)| uri.clone()).collect();
        let contents: Vec<_> = missing.iter().map(|uri| (uri.clone(), self.host.call("workbench", "resolveTextContent", vec![uri.clone()]))).collect();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let mut resolved = Vec::new();
            for (uri, content) in contents {
                resolved.push((uri, content.await.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
            }
            let result = workbench.update_in(cx, |workbench, window, cx| {
                for (uri, content) in resolved {
                    workbench.try_open_document(&uri, &Value::Null, &content?, false, cx)?;
                }
                for (uri, side) in uris.iter().zip(&sides) {
                    if side.scheme == "file" && workbench.editors.model(&side.scheme, &side.path).is_none() {
                        workbench.try_open_document(uri, &Value::Null, &Value::Null, false, cx)?;
                    }
                }
                let data = |input: &Value, side: &Side| {
                    let label = |key: &str| input[key].as_str().map(str::to_string);
                    InputData { side: side.clone(), title: label("title"), description: label("description"), detail: label("detail") }
                };
                let (input1, input2) = (data(&arg["input1"], &sides[1]), data(&arg["input2"], &sides[2]));
                workbench.show_merge_editor(sides[0].clone(), input1, input2, sides[3].clone(), window, cx);
                Ok(Value::Null)
            });
            drop(sender.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `openEditor` of a `MergeEditorInput`: an open merge editor of the same result is activated, else a new one
    /// opens; a result with conflict markers is reset to the auto-merged text (mergeEditorInputModel.ts).
    fn show_merge_editor(&mut self, base: Side, input1: InputData, input2: InputData, result: Side, window: &mut Window, cx: &mut Context<Self>) {
        match self.editors.editors.iter().position(|editor| self.editors.merges.get(&editor.model_id).is_some_and(|merge| merge.result == result)) {
            Some(index) => self.editors.activate(index),
            None => {
                let has_markers = self.editors.model(&result.scheme, &result.path).is_some_and(|model| model.document.lines.iter().any(|line| line.starts_with("<<<<<<<")));
                let id = self.editors.merges.len() + self.editors.editors.len() + 1;
                let input = MergeInput::new(base, input1, input2, result, has_markers);
                // The entry's file name is the tab label (`MergeEditorInput.getName`).
                let entry = TextDocument::from_text(MERGE_EDITOR_SCHEME, Path::new(&format!("/{id}/{}", input.name())), "", "utf8".into(), Some("plaintext"), &self.languages, &self.settings.lock().unwrap());
                self.editors.open_merge(entry, input, false);
            }
        }
        self.marked_text = None;
        self.update_merge_editor(cx);
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// Brings the active merge editor's model up to its models, resets a result opened with conflict markers and
    /// places the result's action zones; other merge editors take theirs back.
    pub(in crate::workbench) fn update_merge_editor(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u32> = self.editors.merges.keys().copied().collect();
        for id in ids {
            let merge = self.editors.merges.get_mut(&id).expect("listed above");
            let (placed, result) = (std::mem::take(&mut merge.placed_zones), merge.result.clone());
            if let Some(model) = self.editors.model_mut(&result.scheme, &result.path) {
                for (zone, _) in placed {
                    model.remove_view_zone(zone);
                }
            }
        }
        if !self.sync_merge() {
            return;
        }
        let merge = self.editors.active_merge_mut().expect("synced above");
        if std::mem::take(&mut merge.reset_pending) {
            self.reset_merge_result(cx);
        }
        let line_height = self.editors.viewport.line_height;
        let merge = self.editors.active_merge().expect("synced above");
        let model = merge.model.as_ref().expect("synced above");
        let starts: Vec<(usize, usize)> = (0..model.ranges.len()).filter(|&index| shows_actions(model, index)).map(|index| (index, model.result_line_range(model.ranges[index].base).start)).collect();
        let result = self.editors.active_mut().expect("the merge editor's result model");
        let placed: Vec<(u32, usize)> = starts.into_iter().map(|(index, start)| (result.add_view_zone(start.checked_sub(2).map(|line| Position::new(line, 0)), line_height), index)).collect();
        self.editors.active_merge_mut().expect("synced above").placed_zones = placed;
    }

    /// Recomputes the active merge editor's ranges when an input changed and reads its states back from a result
    /// that changed (mergeEditorModel.ts autoruns); false without an active merge editor or one of its models.
    fn sync_merge(&mut self) -> bool {
        let Some(id) = self.editors.active_tab().map(|tab| tab.model_id) else { return false };
        let Some(mut merge) = self.editors.merges.remove(&id) else { return false };
        let model = |side: &Side| self.editors.model(&side.scheme, &side.path);
        let synced = if let (Some(base), Some(input1), Some(input2), Some(result)) = (model(&merge.base), model(&merge.input1.side), model(&merge.input2.side), model(&merge.result)) {
            let versions = (base.version_id, input1.version_id, input2.version_id);
            if merge.model.as_ref().is_none_or(|model| model.versions != versions) {
                merge.model = Some(MergeModel::new(versions, base.document.lines.clone(), input1.document.lines.clone(), input2.document.lines.clone()));
            }
            merge.model.as_mut().expect("set above").sync_result(&result.document.lines, result.version_id, true);
            true
        } else {
            false
        };
        self.editors.merges.insert(id, merge);
        synced
    }

    /// Applies a line edit of the active merge editor's model to its result, then takes the result's new diff.
    fn apply_merge_edit(&mut self, edit: LineEdit, cx: &mut Context<Self>) {
        self.run_indenting(
            |editor, _| {
                let (start, end, text) = text_edit(&edit, &editor.document.lines, editor.document.eol);
                editor.execute_edits(vec![(start, end, text, false)], None, EditReason::Unknown("mergeEditor")).into_iter().collect()
            },
            cx,
        );
        let result = self.editors.active().expect("the merge editor's result model");
        let (lines, version) = (result.document.lines.clone(), result.version_id);
        if let Some(model) = self.editors.active_merge_mut().and_then(|merge| merge.model.as_mut()) {
            model.sync_result(&lines, version, false);
        }
        cx.notify();
    }

    /// `MergeEditorViewModel.setState`: the range becomes active and the result takes the state's lines.
    fn set_merge_state(&mut self, index: usize, state: State, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let merge = self.editors.active_merge_mut().expect("synced above");
        merge.active_range = Some(index);
        let edit = merge.model.as_mut().expect("synced above").set_state(index, state);
        self.apply_merge_edit(edit, cx);
    }

    fn run_merge_action(&mut self, action: MergeAction, cx: &mut Context<Self>) {
        match action {
            MergeAction::SetState(index, state) => self.set_merge_state(index, state, cx),
            MergeAction::Ignore(index, input) => {
                if let Some(model) = self.editors.active_merge_mut().and_then(|merge| merge.model.as_mut()) {
                    model.set_input_handled(index, input);
                }
                cx.notify();
            }
        }
    }

    /// `acceptAll`: every range takes `input` too.
    fn accept_all(&mut self, input: Input, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let count = self.active_merge_model().map_or(0, |model| model.ranges.len());
        for index in 0..count {
            let state = self.active_merge_model().expect("synced above").accepted[index].with_input(input, true, false);
            self.set_merge_state(index, state, cx);
        }
    }

    /// `MergeEditorModel.reset` (Reset Result).
    fn reset_merge_result(&mut self, cx: &mut Context<Self>) {
        if !self.sync_merge() {
            return;
        }
        let line_count = self.editors.active().map_or(0, |result| result.document.lines.len());
        let edit = self.editors.active_merge_mut().and_then(|merge| merge.model.as_mut()).expect("synced above").reset(line_count);
        self.apply_merge_edit(edit, cx);
    }

    fn active_merge_model(&self) -> Option<&MergeModel> {
        self.editors.active_merge()?.model.as_ref()
    }

    /// viewModel.ts `activeModifiedBaseRange`: the range at the result's cursor, else the one last acted on.
    fn active_merge_range(&self) -> Option<usize> {
        let model = self.active_merge_model()?;
        let line = self.editors.active()?.selection().head.line + 1;
        let at_cursor = (0..model.ranges.len()).find(|&index| {
            let range = model.result_line_range(model.ranges[index].base);
            range.contains(line) || (range.is_empty() && range.start == line)
        });
        at_cursor.or(self.editors.active_merge()?.active_range)
    }

    /// `toggleActiveConflict`.
    fn toggle_active_conflict(&mut self, input: Input, cx: &mut Context<Self>) -> Outcome {
        self.sync_merge();
        let Some(index) = self.active_merge_range() else { return Outcome::error("There is currently no conflict focused that can be toggled.") };
        let state = self.active_merge_model().expect("found a range").accepted[index].toggle(input);
        self.set_merge_state(index, state, cx);
        Outcome::done()
    }

    /// `goToNextModifiedBaseRange` and `goToPreviousModifiedBaseRange` of the unhandled ranges, in the result: the
    /// cursor goes to the range's first line, revealed near the top; wraps around.
    fn go_to_unhandled_conflict(&mut self, next: bool, cx: &mut Context<Self>) {
        self.sync_merge();
        let Some(model) = self.active_merge_model() else { return };
        let Some(result) = self.editors.active() else { return };
        let line = result.selection().head.line + 1;
        let unhandled: Vec<(usize, LineRange)> = (0..model.ranges.len()).filter(|&index| !model.is_handled(index)).map(|index| (index, model.result_line_range(model.ranges[index].base))).collect();
        let target = match next {
            true => unhandled.iter().find(|(_, range)| range.start > line).or(unhandled.first()),
            false => unhandled.iter().rev().find(|(_, range)| range.end < line).or(unhandled.last()),
        };
        let Some(&(index, range)) = target else { return };
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("found above");
        let target_line = (range.start - 1).min(editor.document.lines.len() - 1);
        let column = editor.document.lines[target_line].chars().position(|c| c != ' ' && c != '\t').unwrap_or(0);
        editor.move_to(Position::new(target_line, column), false);
        editor.reveal_range(Position::new(target_line, 0), Position::new(target_line, 0), VerticalReveal::NearTop, viewport);
        if let Some(merge) = self.editors.active_merge_mut() {
            merge.active_range = Some(index);
        }
        cx.notify();
    }

    /// commands.ts `AcceptMerge` (Complete Merge): saves the result, then closes the merge editor.
    // shortcut: no "Do you want to complete the merge" confirmation with unhandled conflicts; add it with the dialog service.
    fn accept_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(result) = self.editors.active_merge().map(|merge| merge.result.clone()) else { return Outcome::done() };
        let saved = self.save_model(result.scheme.clone(), result.path.clone(), crate::workbench::auto_save::SaveReason::Explicit, cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let saved = saved.await.unwrap_or(false);
            let closed = workbench.update_in(cx, |workbench, window, cx| {
                let index = workbench.editors.editors.iter().position(|editor| workbench.editors.merges.get(&editor.model_id).is_some_and(|merge| merge.result == result));
                if let (true, Some(index)) = (saved, index) {
                    drop(workbench.execute_command("workbench.action.closeActiveEditor", vec![json!({ "editorIndex": index })], window, cx));
                }
            });
            drop(sender.send(closed.map(|_| Value::Null).map_err(|_| exthost::main_thread::serialized_error("Canceled"))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// mergeEditor.ts mixed layout: the inputs side by side on top, the result below.
    pub(super) fn merge_editor_body(&self, merge: &MergeInput, result: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let border = self.color("surface.border");
        let inputs = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.merge_input_pane(merge, Input::One, result, window, cx).flex_1().min_w_0())
            .child(self.merge_input_pane(merge, Input::Two, result, window, cx).flex_1().min_w_0().border_l_1().border_color(border));
        let result_pane = div().flex_1().min_h_0().flex().flex_col().border_t_1().border_color(border).child(self.merge_result_title(merge, result, cx)).child(self.merge_result_area(merge, result, window, cx));
        div().flex_1().min_h_0().flex().flex_col().child(inputs).child(result_pane)
    }

    /// `.code-view > .title`: the label, its description and detail; codicons in them show as their text.
    fn merge_title(&self, title: String, description: Option<String>, detail: Option<String>) -> Div {
        let dim = self.color("descriptionForeground");
        div()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(13.))
            .text_color(self.color("foreground"))
            .child(title)
            .children(description.map(|description| div().text_color(dim).child(description)))
            .children(detail.map(|detail| div().text_color(dim).child(detail)))
    }

    /// resultCodeEditorView.ts title: "Result", the result's path, the remaining conflicts counter, previous and
    /// next conflict, and Complete Merge.
    fn merge_result_title(&self, merge: &MergeInput, result: &OpenEditor, cx: &mut Context<Self>) -> Div {
        let count = merge.model.as_ref().map_or(0, MergeModel::unhandled_count);
        let counter = if count == 1 { "1 Conflict Remaining".to_string() } else { format!("{count} Conflicts Remaining") };
        let path = crate::quick_input::path_label(&result.document.path, None);
        let icon_color = self.color("icon.foreground");
        let mut title = self.merge_title("Result".into(), Some(path), None);
        title = title.child(
            div()
                .id("merge-counter")
                .ml_auto()
                .text_color(self.color("descriptionForeground"))
                .when(count > 0, |counter| counter.cursor_pointer().on_click(cx.listener(|workbench, _, _, cx| workbench.go_to_unhandled_conflict(true, cx))))
                .child(counter),
        );
        for (id, icon, next) in [("merge-previous", "arrow-up", false), ("merge-next", "arrow-down", true)] {
            title = title.child(self.icon(icon, 22., 16., icon_color).id(id).cursor_pointer().rounded(px(5.)).hover(|icon| icon.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(move |workbench, _, _, cx| workbench.go_to_unhandled_conflict(next, cx))));
        }
        title.child(
            div()
                .id("merge-complete")
                .cursor_pointer()
                .px(px(8.))
                .rounded(px(2.))
                .bg(self.color("button.background"))
                .text_color(self.color("button.foreground"))
                .child("Complete Merge")
                .on_click(cx.listener(|workbench, _, window, cx| drop(workbench.execute_command("mergeEditor.acceptMerge", Vec::new(), window, cx)))),
        )
    }

    /// The result's code editor with each range's action row in its view zone and its block border over it.
    fn merge_result_area(&self, merge: &MergeInput, result: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut area = div().flex_1().min_h_0().relative().flex().flex_col().child(self.text_area(result, window, cx));
        let Some(model) = merge.model.as_ref() else { return area };
        let viewport = self.editors.viewport;
        let line_height = viewport.line_height;
        let line_numbers = self.setting("editor.lineNumbers") != json!("off");
        let content_left = EditorMetrics::measure(window, result.document.lines.len(), line_numbers, self.editor_zoom.multiplier()).content_left();
        let top = |index: usize| result.line_top(result.view_line(index), viewport) - result.scroll.top;
        let active = self.active_merge_range();
        for (index, range) in model.ranges.iter().enumerate() {
            let lines = model.result_line_range(range.base);
            if lines.start > result.document.lines.len() + 1 {
                continue;
            }
            let block_top = top(lines.start - 1);
            if merge.placed_zones.iter().any(|placed| placed.1 == index) {
                let row = self.merge_action_row(("merge-result-action", index), result_actions(merge, model, index), cx);
                area = area.child(row.absolute().left(px(content_left)).right_0().top(px(block_top - line_height)).h(px(line_height)));
            }
            if range.is_conflicting() {
                let height = if lines.is_empty() { 2. } else { top(lines.end - 2) + line_height - block_top };
                area = area.child(div().absolute().left(px(content_left)).right_0().top(px(block_top)).h(px(height)).border_1().border_color(self.merge_block_border(model, index, active == Some(index))));
            }
        }
        area
    }

    /// colors.ts `mergeEditor.conflict.*.border` of a range, focused when it is the active range.
    fn merge_block_border(&self, model: &MergeModel, index: usize, focused: bool) -> Rgba {
        let key = match (model.is_handled(index), focused) {
            (false, false) => "mergeEditor.conflict.unhandledUnfocused.border",
            (false, true) => "mergeEditor.conflict.unhandledFocused.border",
            (true, false) => "mergeEditor.conflict.handledUnfocused.border",
            (true, true) => "mergeEditor.conflict.handledFocused.border",
        };
        self.color(key)
    }

    /// conflictActions.ts actions as a codelens-like row: links joined with " | ".
    fn merge_action_row(&self, id: (&'static str, usize), actions: Vec<Action>, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("descriptionForeground");
        let link = self.color("textLink.foreground");
        let font_size = (self.editors.viewport.line_height * 0.6).round();
        let mut row = div().flex().items_center().whitespace_nowrap().text_size(px(font_size)).text_color(foreground);
        for (position, (label, action)) in actions.into_iter().enumerate() {
            if position > 0 {
                row = row.child("\u{a0}|\u{a0}");
            }
            row = match action {
                Some(action) => row.child(
                    div()
                        .id((id.0, id.1 * 16 + position))
                        .cursor_pointer()
                        .hover(move |item| item.text_color(link))
                        .on_click(cx.listener(move |workbench, _, _, cx| workbench.run_merge_action(action, cx)))
                        .child(label),
                ),
                None => row.child(label),
            };
        }
        row
    }

    /// inputCodeEditorView.ts: the input's title bar over its read-only lines, scrolled with the result.
    fn merge_input_pane(&self, merge: &MergeInput, input: Input, result: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let data = if input == Input::One { &merge.input1 } else { &merge.input2 };
        let fallback = if input == Input::One { "Input 1" } else { "Input 2" };
        let title = self.merge_title(data.title.clone().unwrap_or_else(|| fallback.into()), data.description.clone(), data.detail.clone());
        let pane = div().flex().flex_col().min_h_0().child(title);
        let (Some(editor), Some(model)) = (self.editors.model(&data.side.scheme, &data.side.path), merge.model.as_ref()) else { return pane };
        pane.child(self.merge_input_lines(merge, model, input, editor, result, window, cx))
    }

    fn merge_input_lines(&self, merge: &MergeInput, model: &MergeModel, input: Input, editor: &OpenEditor, result: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let metrics = EditorMetrics::measure(window, editor.document.lines.len(), true, self.editor_zoom.multiplier());
        let line_height = metrics.line_height;
        let action_rows: Vec<(usize, Vec<Action>)> = (0..model.ranges.len()).filter(|&index| shows_actions(model, index)).map(|index| (index, input_actions(merge, model, input, index))).filter(|(_, actions)| !actions.is_empty()).collect();
        let action_starts: Vec<usize> = action_rows.iter().map(|(index, _)| model.ranges[*index].range(input).start).collect();
        let line_y = |line: usize| ((line - 1) + action_starts.iter().filter(|start| **start <= line).count()) as f32 * line_height;
        // scrollSynchronizer.ts: the input line beside the result's first visible line is at the same height.
        let viewport = self.editors.viewport;
        let first_view_line = result.view_line_at(result.scroll.top, viewport);
        let first_result_line = result.model_line(first_view_line) + 1;
        let offset = result.scroll.top - result.line_top(first_view_line, viewport);
        let scroll_top = line_y(model.input_line_for_result_line(input, first_result_line).min(editor.document.lines.len())) + offset - viewport.top_whitespace;
        let height = viewport.height;
        let content_left = metrics.glyph_margin_width() + metrics.line_numbers_width + metrics.decorations_width;
        let active = self.active_merge_range();
        let mut rows = Vec::new();
        let mut start = 0;
        while start + 1 < editor.document.lines.len() && line_y(start + 2) - scroll_top < 0. {
            start += 1;
        }
        for index in start..editor.document.lines.len() {
            let top = line_y(index + 1) - scroll_top;
            if top > height {
                break;
            }
            rows.push(self.merge_input_line(model, input, editor, index, metrics).absolute().top(px(top)).left_0().right_0());
        }
        let mut action_rows = action_rows.into_iter().peekable();
        for (index, range) in model.ranges.iter().enumerate() {
            let lines = range.range(input);
            let block_top = line_y(lines.start) - scroll_top;
            let actions = action_rows.next_if(|(action_index, _)| *action_index == index).map(|(_, actions)| actions);
            if block_top + line_height * (lines.len() as f32) < 0. || block_top > height {
                continue;
            }
            if let Some(actions) = actions {
                let row = self.merge_action_row(if input == Input::One { ("merge-input1-action", index) } else { ("merge-input2-action", index) }, actions, cx);
                rows.push(row.absolute().left(px(content_left)).right_0().top(px(block_top - line_height)).h(px(line_height)));
            }
            if range.is_conflicting() {
                let height = if lines.is_empty() { 2. } else { lines.len() as f32 * line_height };
                rows.push(div().absolute().left(px(content_left)).right_0().top(px(block_top)).h(px(height)).border_1().border_color(self.merge_block_border(model, index, active == Some(index))));
            }
        }
        div()
            .id(if input == Input::One { "merge-input1" } else { "merge-input2" })
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .font_family(super::EDITOR_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(line_height))
            .bg(self.color("editor.background"))
            .text_color(self.color("editor.foreground"))
            .on_scroll_wheel(cx.listener(|workbench, event: &ScrollWheelEvent, window, cx| workbench.editor_wheel(event, window, cx)))
            .children(rows)
    }

    /// One read-only input line: number, `mergeEditor.change.background` on changed lines and
    /// `mergeEditor.change.word.background` on their changed characters, text.
    fn merge_input_line(&self, model: &MergeModel, input: Input, editor: &OpenEditor, index: usize, metrics: EditorMetrics) -> Div {
        let line_number = index + 1;
        let text = &editor.document.lines[index];
        let tab_size = editor.document.indentation.tab_size;
        let mut content = div().absolute().top_0().bottom_0().right_0().left(px(metrics.glyph_margin_width() + metrics.line_numbers_width + metrics.decorations_width));
        let change = model.ranges.iter().flat_map(|range| range.diffs(input)).find(|diff| diff.modified.contains(line_number));
        if let Some(change) = change {
            content = content.child(div().absolute().inset_0().bg(self.color("mergeEditor.change.background")));
            let word = self.color("mergeEditor.change.word.background");
            for (left, width) in char_ranges(change, ZoneSide::Modified, line_number, text, tab_size, metrics.char_width) {
                content = content.child(div().absolute().top_0().h_full().left(px(left)).w(px(width)).bg(word));
            }
        }
        let highlights = editor.document.tokens.highlights(&self.theme, &self.languages, &editor.document.lines, index, tab_size);
        content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
        div()
            .h(px(metrics.line_height))
            .flex()
            .child(div().w(px(metrics.glyph_margin_width())).flex_none())
            .child(div().w(px(metrics.line_numbers_width)).flex_none().text_right().text_color(self.color("editorLineNumber.foreground")).child(line_number.to_string()))
            .child(content)
    }
}

/// conflictActions.ts: a non-conflicting range that is handled shows no actions (`showNonConflictingChanges` off).
fn shows_actions(model: &MergeModel, index: usize) -> bool {
    model.ranges[index].is_conflicting() || !model.is_handled(index)
}

fn input_title(merge: &MergeInput, input: Input) -> String {
    let (data, fallback) = if input == Input::One { (&merge.input1, "Input 1") } else { (&merge.input2, "Input 2") };
    data.title.clone().unwrap_or_else(|| fallback.into())
}

/// conflictActions.ts `getItemsInput`: Accept (or Append), Accept Combination, Ignore.
fn input_actions(merge: &MergeInput, model: &MergeModel, input: Input, index: usize) -> Vec<Action> {
    let state = model.accepted[index];
    let mut actions = Vec::new();
    if state == State::Unrecognized || state.includes(input) {
        return actions;
    }
    let title = input_title(merge, input);
    let other = input.other();
    // `shouldUseAppendInsteadOfAccept` is off by default: the other input's acceptance shows Accept, not Append.
    actions.push((format!("Accept {title}"), Some(MergeAction::SetState(index, state.with_input(input, true, false)))));
    if model.can_be_combined(index) {
        let label = if model.is_order_relevant(index) { format!("Accept Combination ({title} First)") } else { "Accept Combination".into() };
        actions.push((label, Some(MergeAction::SetState(index, State::Base.with_input(input, true, false).with_input(other, true, true)))));
    }
    let handled = model.handled[index];
    if !(if input == Input::One { handled.0 } else { handled.1 }) {
        actions.push(("Ignore".into(), Some(MergeAction::Ignore(index, input))));
    }
    actions
}

/// conflictActions.ts `resultItems`: what the range takes, Remove per included input, Reset to base.
fn result_actions(merge: &MergeInput, model: &MergeModel, index: usize) -> Vec<Action> {
    let state = model.accepted[index];
    let mut actions: Vec<Action> = Vec::new();
    let mut inputs = vec![Input::One, Input::Two];
    if let State::Both { first: Input::Two, .. } = state {
        inputs.reverse();
    }
    match state {
        State::Unrecognized => actions.push(("Manual Resolution".into(), None)),
        State::Base => actions.push(("No Changes Accepted".into(), None)),
        _ => actions.push((inputs.iter().filter(|input| state.includes(**input)).map(|input| input_title(merge, *input)).collect::<Vec<_>>().join(" + "), None)),
    }
    for input in inputs {
        if state.includes(input) {
            actions.push((format!("Remove {}", input_title(merge, input)), Some(MergeAction::SetState(index, state.with_input(input, false, false)))));
        }
    }
    if state == State::Unrecognized {
        actions.push(("Reset to base".into(), Some(MergeAction::SetState(index, State::Base))));
    }
    actions
}

#[cfg(test)]
mod tests;
