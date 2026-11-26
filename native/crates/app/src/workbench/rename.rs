//! rename.ts and renameWidget.ts: F2 asks the providers where the symbol at the cursor is (`prepareRename`), shows the
//! rename input over it, and applies the providers' workspace edit for the typed name.

use futures::channel::oneshot;
use gpui::{AnyElement, BoxShadow, Context, FocusHandle, Task, Window, deferred, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_message::{MessagePart, horizontal_in_page};
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::{Outcome, error_value};
use crate::editor::{LINE_HEIGHT, OpenEditor, Position, VerticalReveal, char_column, utf16_column, word_at};
use crate::input_box::InputBox;
use crate::language_features::Candidate;
use crate::notifications::{Notification, Severity};

pub(super) const REGISTER: &str = "$registerRenameSupport";
const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;
/// `RenameCandidateView.getLayoutInfo`: a candidate row's height is the line height and 2px above and below.
const CANDIDATE_PADDING: f32 = 2.;

pub(super) struct Rename {
    /// The running `prepareRename` or rename edits request; dropping it cancels the host calls.
    request: Option<Task<()>>,
    pub(super) widget: Option<RenameWidget>,
    pub(super) view: InputView,
    pub(super) focus: FocusHandle,
}

impl Rename {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Rename {
        Rename { request: None, widget: None, view: InputView::default(), focus: cx.focus_handle() }
    }
}

/// The input of one `getInput`, over the symbol's range.
pub(super) struct RenameWidget {
    model_id: u32,
    version_id: u32,
    start: Position,
    end: Position,
    /// The cursor when F2 ran, which the edits are asked at.
    position: Position,
    current_name: String,
    /// The ordered providers and the one whose location was taken (`_providerRenameIdx`).
    handles: Vec<u64>,
    provider_index: usize,
    pub(super) input: InputBox,
}

/// A resolved rename location: its range, its text, the providers' reject reasons.
struct Location {
    start: Position,
    end: Position,
    text: String,
    reject_reason: Option<String>,
}

impl Workbench {
    pub(super) fn register_rename(&mut self) {
        self.commands.register("editor.action.rename", |workbench, _, window, cx| {
            workbench.run_rename(window, cx);
            Outcome::done()
        });
        // shortcut: no Refactor Preview (the bulk edit preview handler of workbench/contrib/bulkEdit), so
        // `supportPreview` is false and Cmd+Enter renames as Enter does; show the preview once that row has it.
        for id in ["acceptRenameInput", "acceptRenameInputWithPreview"] {
            self.commands.register(id, |workbench, _, window, cx| {
                workbench.accept_rename_input(window, cx);
                Outcome::done()
            });
        }
        self.commands.register("cancelRenameInput", |workbench, _, window, cx| {
            workbench.cancel_rename_input(true, window, cx);
            Outcome::done()
        });
        // No `NewSymbolNamesProvider` runs (AI rename suggestions are removed), so the list never has a candidate
        // and both move back to the current name.
        for id in ["focusNextRenameSuggestion", "focusPreviousRenameSuggestion"] {
            self.commands.register(id, |workbench, _, _, cx| {
                if let Some(widget) = workbench.rename.widget.as_mut() {
                    widget.input.value = widget.current_name.clone();
                    cx.notify();
                }
                Outcome::done()
            });
        }
        self.commands.register("_executeDocumentRenameProvider", |workbench, args, _, cx| workbench.execute_document_rename_provider(&args, cx));
        self.commands.register("_executePrepareRename", |workbench, args, _, cx| workbench.execute_prepare_rename(&args, cx));
    }

    /// `editorHasRenameProvider` and `renameInputVisible`.
    pub(super) fn rename_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(editor) = self.editors.active() else { return };
        context.insert("editorHasRenameProvider".into(), json!(!self.rename_handles(editor).is_empty()));
        context.insert("renameInputVisible".into(), json!(self.rename.widget.is_some()));
    }

    pub(super) fn rename_input_focused(&self, window: &Window) -> bool {
        self.rename.widget.is_some() && self.rename.focus.is_focused(window)
    }

    fn rename_handles(&self, editor: &OpenEditor) -> Vec<u64> {
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect()
    }

    /// `RenameController.run`: the location from `prepareRename` or the word at the cursor, a reject reason as an
    /// editor message, else the input.
    fn run_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename.request = None;
        self.cancel_rename_input(false, window, cx);
        let Some(editor) = self.editors.active() else { return };
        if self.active_editor_readonly().is_some() {
            return;
        }
        let handles = self.rename_handles(editor);
        if handles.is_empty() {
            return;
        }
        let position = editor.selection().head;
        let (model_id, version_id) = (editor.model_id, editor.text_model().version_id);
        let resolve = self.resolve_rename_location(editor, &handles, position);
        let operation = self.start_cancellable_operation();
        self.rename.request = Some(cx.spawn_in(window, async move |workbench, cx| {
            let resolved = resolve.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.rename.request = None;
                let Ok((location, provider_index)) = resolved else { return };
                if super::editor_state::is_cancelled(&operation) {
                    return;
                }
                let Some(editor) = workbench.editors.active() else { return };
                // EditorStateCancellationTokenSource(Position | Value).
                if (editor.model_id, editor.text_model().version_id) != (model_id, version_id) || editor.selection().head != position {
                    return;
                }
                if let Some(reason) = location.reject_reason {
                    return workbench.show_editor_message(vec![MessagePart::text(reason)], cx);
                }
                workbench.show_rename_input(location, position, handles, provider_index, window, cx);
            });
        }));
    }

    /// `RenameSkeleton.resolveRenameLocation`: the providers that can resolve in order, stopping at the first that
    /// cannot; none answering takes the word at the position. A failed call ends the run (`Err`).
    fn resolve_rename_location(&self, editor: &OpenEditor, handles: &[u64], position: Position) -> impl Future<Output = Result<(Location, usize), ()>> + use<> {
        let uri = editor.document().uri();
        let lines = editor.document().lines.clone();
        let supports_resolve: Vec<bool> = handles.iter().map(|handle| self.language_features.registration(*handle).is_some_and(|registration| registration.args.get(2) == Some(&json!(true)))).collect();
        let call = self.host.caller();
        let handles = handles.to_vec();
        async move {
            let mut rejects = Vec::new();
            for (index, handle) in handles.iter().enumerate() {
                if !supports_resolve[index] {
                    break;
                }
                let args = vec![json!(handle), uri.clone(), position_value(&lines, position)];
                let answer = call("ExtHostLanguageFeatures", "$resolveRenameLocation", args).await.map_err(drop)?.map_err(drop)?;
                if answer.is_null() {
                    continue;
                }
                if let Some(reason) = answer["rejectReason"].as_str() {
                    rejects.push(reason.to_owned());
                    continue;
                }
                let (start, end) = range_positions(&lines, &answer["range"]);
                return Ok((Location { start, end, text: answer["text"].as_str().unwrap_or_default().to_owned(), reject_reason: None }, index));
            }
            let reject_reason = (!rejects.is_empty()).then(|| rejects.join("\n"));
            let location = match word_at(&lines[position.line], position.column) {
                Some(word) => {
                    let text = lines[position.line].chars().skip(word.start).take(word.len()).collect();
                    Location { start: Position::new(position.line, word.start), end: Position::new(position.line, word.end), text, reject_reason }
                }
                None => Location { start: position, end: position, text: String::new(), reject_reason },
            };
            Ok((location, 0))
        }
    }

    /// `RenameWidget.getInput` and `_show`: the name selected as far as the editor's one-line selection inside the
    /// range covers it, the line revealed centered when outside the viewport.
    fn show_rename_input(&mut self, location: Location, position: Position, handles: Vec<u64>, provider_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut() else { return };
        let selection = editor.selection();
        let (start, end) = (selection.start(), selection.end());
        let name_length = location.text.chars().count();
        let (select_start, select_end) = match start != end && start.line == end.line && location.start <= start && end <= location.end {
            true => (start.column - location.start.column, end.column.min(location.end.column) - location.start.column),
            false => (0, name_length),
        };
        let mut input = InputBox::new(location.text.clone());
        input.select(select_start, select_end);
        let scroll = editor.scroll;
        editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        let scrolled = editor.scroll != scroll;
        self.rename.widget = Some(RenameWidget {
            model_id: editor.model_id,
            version_id: editor.text_model().version_id,
            start: location.start,
            end: location.end,
            position,
            current_name: location.text,
            handles,
            provider_index,
            input,
        });
        self.rename.view.reset();
        window.focus(&self.rename.focus, cx);
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// `_currentCancelInput`: the input goes; the editor takes the focus back when asked to.
    pub(super) fn cancel_rename_input(&mut self, focus_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.widget.take().is_none() {
            return;
        }
        if focus_editor {
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// On each render: another model, a changed value or a cursor outside the symbol's range cancels the input
    /// (`EditorStateCancellationTokenSource` with the location's range).
    pub(super) fn follow_rename_editor_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = &self.rename.widget else { return };
        let holds = self.editors.active().is_some_and(|editor| {
            let head = editor.selection().head;
            (editor.model_id, editor.text_model().version_id) == (widget.model_id, widget.version_id) && widget.start <= head && head <= widget.end
        });
        if !holds {
            self.cancel_rename_input(true, window, cx);
        }
    }

    /// `onDidBlurEditorWidget`: the focus left the editor for another part of the window, which keeps it.
    pub(super) fn rename_editor_blurred(&mut self) {
        self.rename.widget = None;
    }

    /// `_currentAcceptInput` and the rest of `run`: an unchanged or blank name cancels; otherwise the edits are asked
    /// for, a reject reason is an info notification, and the edit applies with the selection collapsed.
    fn accept_rename_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.rename.widget.take() else { return };
        window.focus(&self.editor_focus, cx);
        cx.notify();
        let new_name = widget.input.value.clone();
        if new_name == widget.current_name || new_name.trim().is_empty() {
            return;
        }
        let Some(editor) = self.editors.active() else { return };
        if (editor.model_id, editor.text_model().version_id) != (widget.model_id, widget.version_id) {
            return;
        }
        let uri = editor.document().uri();
        let position = position_value(&editor.document().lines, widget.position);
        let calls: Vec<_> = widget.handles[widget.provider_index..]
            .iter()
            .map(|handle| (*handle, vec![json!(handle), uri.clone(), position.clone(), json!(new_name)]))
            .collect();
        let call = self.host.caller();
        self.rename.request = Some(cx.spawn(async move |workbench, cx| {
            let mut rejects = Vec::new();
            let mut result = None;
            for (_, args) in calls {
                match call("ExtHostLanguageFeatures", "$provideRenameEdits", args).await {
                    Ok(Ok(Value::Null)) => rejects.push("No result.".to_owned()),
                    Ok(Ok(edit)) if edit["rejectReason"].is_string() => rejects.push(edit["rejectReason"].as_str().unwrap_or_default().to_owned()),
                    Ok(Ok(edit)) => {
                        result = Some(Ok(edit));
                        break;
                    }
                    Ok(Err(_)) => {
                        result = Some(Err(()));
                        break;
                    }
                    Err(_) => return,
                }
            }
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.rename.request = None;
                match result {
                    None => {
                        workbench.notify(Notification::new(Severity::Info, rejects.join("\n")));
                    }
                    Some(Err(())) => workbench.notify_error("Rename failed to compute edits"),
                    Some(Ok(edit)) => workbench.apply_rename_edit(&edit, widget.model_id, cx),
                }
                cx.notify();
            });
        }));
    }

    fn apply_rename_edit(&mut self, edit: &Value, model_id: u32, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.model_id == model_id) else { return };
        let head = editor.selection().head;
        editor.set_selections(vec![crate::editor::Selection::caret(head)]);
        // shortcut: only the text edits of the workspace edit apply, as for code actions; file edits need the bulk
        // edit service on the workbench side.
        let text_edits: Vec<Value> = edit["edits"].as_array().into_iter().flatten().filter(|edit| edit.get("textEdit").is_some()).cloned().collect();
        if self.apply_workspace_text_edits(&Value::Array(text_edits), &Value::Null, files::operations::new_undo_group(), cx).is_err() {
            self.notify_error("Rename failed to apply edits");
        }
    }

    /// `rename` for `vscode.executeDocumentRenameProvider`: the edit of the first provider that answers.
    fn execute_document_rename_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(error_value("Illegal argument")));
        };
        let Some(new_name) = args.get(2).and_then(Value::as_str) else {
            return Outcome::Ready(Err(error_value("Illegal argument")));
        };
        let uri = editor.document().uri();
        let position = args.get(1).cloned().unwrap_or_default();
        let calls: Vec<_> = self
            .rename_handles(editor)
            .into_iter()
            .map(|handle| vec![json!(handle), uri.clone(), position.clone(), json!(new_name)])
            .collect();
        let call = self.host.caller();
        let (sender, receiver) = oneshot::channel();
        cx.background_spawn(async move {
            let mut rejects = Vec::new();
            for args in calls {
                match call("ExtHostLanguageFeatures", "$provideRenameEdits", args).await {
                    Ok(Ok(Value::Null)) => rejects.push("No result.".to_owned()),
                    Ok(Ok(edit)) if edit["rejectReason"].is_string() => rejects.push(edit["rejectReason"].as_str().unwrap_or_default().to_owned()),
                    Ok(result) => {
                        let _ = sender.send(result);
                        return;
                    }
                    Err(_) => return,
                }
            }
            let _ = sender.send(Ok(json!({ "edits": [], "rejectReason": rejects.join("\n") })));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_executePrepareRename`: the location, a reject reason thrown as an error.
    fn execute_prepare_rename(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(error_value("Illegal argument")));
        };
        let lines = &editor.document().lines;
        let position = args.get(1).cloned().unwrap_or_default();
        let position = position_at(lines, &position["lineNumber"], &position["column"]);
        let resolve = self.resolve_rename_location(editor, &self.rename_handles(editor), position);
        let lines = lines.clone();
        let (sender, receiver) = oneshot::channel();
        cx.background_spawn(async move {
            let result = match resolve.await {
                Err(()) => Err(error_value("Canceled")),
                Ok((location, _)) => match location.reject_reason {
                    Some(reason) => Err(error_value(&reason)),
                    None => Ok(json!({ "range": range_value(&lines, location.start, location.end), "text": location.text })),
                },
            };
            let _ = sender.send(result);
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// The rename box (renameWidget.css with `_updateStyles`): the input with its button row at the symbol's start,
    /// below the line when more than six candidate rows fit under it in the window, else above.
    pub(super) fn rename_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.rename.widget.as_ref().filter(|widget| widget.model_id == editor.model_id)?;
        let viewport = self.editors.viewport;
        let view_line = editor.position_view_line(widget.start, false);
        let segment = editor.segment(view_line);
        let left = editor.segment_visible_column(segment, widget.start.column) * char_width - editor.scroll.left + content_left;
        let line_top = editor.line_top(view_line, viewport) - editor.scroll.top;
        let editor_bounds = self.text_area_bounds.get();
        let window_size = window.viewport_size().map(f32::from);
        let zoom = viewport.line_height / LINE_HEIGHT;
        // .rename-box: a 1px border around the 26px input row, at least 200px wide.
        let (width, height) = (202., 28.);
        let below_available = window_size.height - (editor_bounds.origin.y + line_top);
        let candidate_height = viewport.line_height + 2. * CANDIDATE_PADDING;
        let fits_below = editor_bounds.origin.y + line_top + viewport.line_height + height <= window_size.height;
        let fits_above = editor_bounds.origin.y + line_top - height >= 0.;
        let below = match below_available > candidate_height * 6. {
            true => fits_below || !fits_above,
            false => !fits_above && fits_below,
        };
        let top = if below { line_top + viewport.line_height } else { line_top - height };
        let left = horizontal_in_page(left, width, editor_bounds, window_size);
        let focused = self.rename.focus.is_focused(window);
        let input_border = self.theme.color("input.border");
        let row_border = if focused { Some(self.color("focusBorder")) } else { input_border };
        let row = div()
            .w(px(200.))
            .h(px(26.))
            .p(px(3.))
            .when_some(row_border, |row, color| row.border_1().border_color(color))
            .when(row_border.is_none(), |row| row.p(px(4.)))
            .rounded(px(4.))
            .bg(self.color("input.background"))
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(184.))
                    .h(px(18. * zoom))
                    .flex()
                    .font_family(EDITOR_FONT)
                    .text_size(px(EDITOR_FONT_SIZE * zoom))
                    .line_height(px(LINE_HEIGHT * zoom))
                    .child(self.input_text(InputId::Rename, InputLook { password: false, placeholder: None, focused }, cx)),
            );
        let shadow = self.theme.color("widget.shadow");
        let border = self.theme.color("widget.border");
        let element = div()
            .id("rename-box")
            .absolute()
            .left(px(left))
            .top(px(top))
            .min_w(px(width))
            .when_some(border, |element, color| element.border_1().border_color(color))
            .rounded(px(8.))
            .bg(self.color("editorWidget.background"))
            .text_color(self.foreground_or("input.foreground"))
            .when_some(shadow, |element, color| element.shadow(vec![BoxShadow { color: color.into(), offset: point(px(0.), px(0.)), blur_radius: px(8.), spread_radius: px(2.), inset: false }]))
            .track_focus(&self.rename.focus)
            .cursor_default()
            .occlude()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(row);
        Some(deferred(element).with_priority(1).into_any_element())
    }
}

/// `IPosition` with a 1-based UTF-16 column.
fn position_value(lines: &[String], position: Position) -> Value {
    json!({ "lineNumber": position.line + 1, "column": utf16_column(&lines[position.line], position.column) + 1 })
}

fn range_value(lines: &[String], start: Position, end: Position) -> Value {
    json!({
        "startLineNumber": start.line + 1,
        "startColumn": utf16_column(&lines[start.line], start.column) + 1,
        "endLineNumber": end.line + 1,
        "endColumn": utf16_column(&lines[end.line], end.column) + 1,
    })
}

/// An `IRange`'s start and end as char positions, clamped to the lines.
fn range_positions(lines: &[String], range: &Value) -> (Position, Position) {
    (position_at(lines, &range["startLineNumber"], &range["startColumn"]), position_at(lines, &range["endLineNumber"], &range["endColumn"]))
}

fn position_at(lines: &[String], line: &Value, column: &Value) -> Position {
    let line = (line.as_u64().unwrap_or(1).max(1) as usize - 1).min(lines.len() - 1);
    let column = column.as_u64().unwrap_or(1).max(1) as usize - 1;
    Position::new(line, char_column(&lines[line], column))
}

#[cfg(test)]
mod tests;
