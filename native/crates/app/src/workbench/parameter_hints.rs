//! parameterHints.ts, parameterHintsModel.ts, parameterHintsWidget.ts and provideSignatureHelp.ts: signature help
//! above the cursor, triggered by Cmd+Shift+Space, the providers' trigger characters, and kept up to date while the
//! cursor moves or the text changes.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use futures::channel::oneshot;
use gpui::{AnyElement, Bounds, BoxShadow, Context, FontWeight, MouseButton, Size, Task, Window, canvas, deferred, div, point, prelude::*, px, rgba};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_message::horizontal_in_page;
use super::markdown_view::MarkdownStyle;
use crate::commands::Outcome;
use crate::editor::{LINE_HEIGHT, OpenEditor, utf16_column, visible_column};
use crate::language_features::Candidate;
use crate::markdown::{MarkdownString, parse};

const REGISTER: &str = "$registerSignatureHelpProvider";
/// `ParameterHintsModel.DEFAULT_DELAY`.
const DELAY: Duration = Duration::from_millis(120);
const INVOKE: u64 = 1;
const TRIGGER_CHARACTER: u64 = 2;
const CONTENT_CHANGE: u64 = 3;
const UI_FONT: &str = ".SystemUIFont";
const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;
/// cornerRadius-large.
const RADIUS: f32 = 8.;

#[derive(Default)]
pub(super) struct ParameterHints {
    state: State,
    /// `_pendingTriggers`: (triggerKind, triggerCharacter).
    pending: Vec<(u64, Option<String>)>,
    trigger_id: usize,
    /// The throttled delayer's timer; `Some` while a trigger waits (`isTriggered`).
    delayer: Option<Task<()>>,
    /// The model the hints are for.
    model_id: Option<u32>,
    widget_bounds: Rc<Cell<Option<Bounds<f32>>>>,
}

#[derive(Default)]
enum State {
    #[default]
    Default,
    /// The request (dropping it cancels the provider calls), and the hints active before it (`previouslyActiveHints`).
    Pending(#[allow(dead_code)] Task<()>, Option<Value>),
    Active(Value),
}

impl Workbench {
    pub(super) fn register_parameter_hints(&mut self) {
        self.commands.register("editor.action.triggerParameterHints", |workbench, _, _, cx| {
            workbench.trigger_parameter_hints(INVOKE, None, Duration::ZERO, cx);
            Outcome::done()
        });
        self.commands.register("closeParameterHints", |workbench, _, _, cx| {
            workbench.cancel_parameter_hints(cx);
            Outcome::done()
        });
        self.commands.register("showPrevParameterHint", |workbench, _, _, cx| {
            workbench.cycle_parameter_hints(false, cx);
            Outcome::done()
        });
        self.commands.register("showNextParameterHint", |workbench, _, _, cx| {
            workbench.cycle_parameter_hints(true, cx);
            Outcome::done()
        });
        self.commands.register("_executeSignatureHelpProvider", |workbench, args, _, cx| workbench.execute_signature_help_provider(&args, cx));
    }

    /// `parameterHintsVisible`, `parameterHintsMultipleSignatures` and `editorHasSignatureHelpProvider`.
    pub(super) fn parameter_hints_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let hints = self.active_parameter_hints();
        context.insert("parameterHintsVisible".into(), json!(hints.is_some()));
        context.insert("parameterHintsMultipleSignatures".into(), json!(hints.is_some_and(|hints| signatures(hints).len() > 1)));
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        context.insert("editorHasSignatureHelpProvider".into(), json!(!self.language_features.ordered(REGISTER, &candidate).is_empty()));
    }

    fn active_parameter_hints(&self) -> Option<&Value> {
        match &self.parameter_hints.state {
            State::Active(hints) => Some(hints),
            _ => None,
        }
    }

    fn parameter_hints_triggered(&self) -> bool {
        !matches!(self.parameter_hints.state, State::Default) || self.parameter_hints.delayer.is_some()
    }

    /// `trigger`: the context queues and the throttled delayer runs `doTrigger` after `delay`.
    fn trigger_parameter_hints(&mut self, kind: u64, character: Option<String>, delay: Duration, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        if self.language_features.ordered(REGISTER, &candidate).is_empty() {
            return;
        }
        self.parameter_hints.trigger_id += 1;
        let trigger_id = self.parameter_hints.trigger_id;
        self.parameter_hints.pending.push((kind, character));
        // `Delayer.trigger`: a pending timer keeps its deadline unless the new delay is shorter.
        if self.parameter_hints.delayer.is_some() && delay == DELAY {
            return;
        }
        self.parameter_hints.delayer = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.parameter_hints.delayer = None;
                workbench.do_trigger_parameter_hints(trigger_id, cx);
            });
        }));
    }

    /// `doTrigger`: the merged context goes to the providers in order until one answers.
    fn do_trigger_parameter_hints(&mut self, trigger_id: usize, cx: &mut Context<Self>) {
        let is_retrigger = !matches!(self.parameter_hints.state, State::Default);
        let active = match std::mem::take(&mut self.parameter_hints.state) {
            State::Active(hints) => Some(hints),
            State::Pending(_, previous) => previous,
            State::Default => None,
        };
        let pending = std::mem::take(&mut self.parameter_hints.pending);
        let Some((kind, character)) = pending.into_iter().reduce(|previous, current| if current.0 == CONTENT_CHANGE { previous } else { current }) else { return };
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let head = editor.selection().head;
        let position = json!({ "lineNumber": head.line + 1, "column": utf16_column(&editor.document().lines[head.line], head.column) + 1 });
        let context = json!({ "triggerKind": kind, "triggerCharacter": character, "isRetrigger": is_retrigger, "activeSignatureHelp": active });
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect();
        let model_id = editor.model_id;
        let request = self.provide_signature_help(handles, uri, position, context, cx);
        let task = cx.spawn(async move |workbench, cx| {
            let result = request.await;
            let _ = workbench.update(cx, |workbench, cx| workbench.parameter_hints_answered(trigger_id, model_id, result, cx));
        });
        self.parameter_hints.state = State::Pending(task, active);
    }

    /// provideSignatureHelp.ts: the first provider with an answer wins; its result is released right away, as the
    /// widget keeps only the value.
    fn provide_signature_help(&self, handles: Vec<u64>, uri: Value, position: Value, context: Value, cx: &mut Context<Self>) -> Task<Option<Value>> {
        let calls: Vec<_> = handles.iter().map(|&handle| (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideSignatureHelp", vec![json!(handle), uri.clone(), position.clone(), context.clone()]))).collect();
        cx.spawn(async move |workbench, cx| {
            for (handle, call) in calls {
                let Ok(Ok(result)) = call.await else { continue };
                if result.is_null() {
                    continue;
                }
                if let Some(id) = result.get("id").cloned() {
                    let _ = workbench.update(cx, |workbench, _| workbench.host.send("ExtHostLanguageFeatures", "$releaseSignatureHelp", vec![json!(handle), id]));
                }
                return Some(result);
            }
            None
        })
    }

    fn parameter_hints_answered(&mut self, trigger_id: usize, model_id: u32, result: Option<Value>, cx: &mut Context<Self>) {
        if trigger_id != self.parameter_hints.trigger_id {
            return;
        }
        match result.filter(|hints| !signatures(hints).is_empty()) {
            Some(hints) => {
                self.parameter_hints.state = State::Active(hints);
                self.parameter_hints.model_id = Some(model_id);
            }
            None => self.parameter_hints.state = State::Default,
        }
        cx.notify();
    }

    /// `cancel`: the state resets and the delayer stops.
    pub(super) fn cancel_parameter_hints(&mut self, cx: &mut Context<Self>) {
        let was_shown = self.active_parameter_hints().is_some();
        self.parameter_hints.state = State::Default;
        self.parameter_hints.delayer = None;
        if was_shown {
            cx.notify();
        }
    }

    /// `next` and `previous`: the active signature moves, wrapping with `editor.parameterHints.cycle`; without
    /// another signature the hints close.
    fn cycle_parameter_hints(&mut self, forward: bool, cx: &mut Context<Self>) {
        let cycle = self.parameter_hints_setting("cycle") != json!(false);
        let State::Active(hints) = &mut self.parameter_hints.state else { return };
        let length = signatures(hints).len();
        let active = hints["activeSignature"].as_u64().unwrap_or(0) as usize;
        let edge = if forward { active % length == length - 1 } else { active == 0 };
        if (length < 2 || edge) && !cycle {
            return self.cancel_parameter_hints(cx);
        }
        let next = match (forward, edge) {
            (true, true) => 0,
            (true, false) => active + 1,
            (false, true) => length - 1,
            (false, false) => active - 1,
        };
        hints["activeSignature"] = json!(next);
        cx.notify();
    }

    fn parameter_hints_setting(&self, key: &str) -> Value {
        let Some(editor) = self.editors.active() else { return Value::Null };
        self.suggest_setting(editor, &format!("editor.parameterHints.{key}"))
    }

    /// `onDidType`: a trigger character, or a retrigger character while triggered, asks again.
    pub(super) fn parameter_hints_typed(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.parameter_hints_setting("enabled") == json!(false) {
            return;
        }
        let Some(last) = text.chars().last() else { return };
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let (mut trigger, mut retrigger) = (false, false);
        for registration in self.language_features.ordered(REGISTER, &candidate) {
            let metadata = registration.args.get(2).unwrap_or(&Value::Null);
            let has = |key: &str| metadata[key].as_array().into_iter().flatten().filter_map(Value::as_str).any(|character| character.chars().next() == Some(last));
            trigger |= has("triggerCharacters");
            retrigger |= has("triggerCharacters") || has("retriggerCharacters");
        }
        if trigger || self.parameter_hints_triggered() && retrigger {
            self.trigger_parameter_hints(TRIGGER_CHARACTER, Some(last.to_string()), DELAY, cx);
        }
    }

    /// `onCursorChange` and `onModelContentChange`: a mouse move cancels; any change while triggered asks again;
    /// another model cancels (`onModelChanged`).
    pub(super) fn parameter_hints_editor_changed(&mut self, source: &str, cx: &mut Context<Self>) {
        if !self.parameter_hints_triggered() {
            return;
        }
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if source == "mouse" || model_id.is_none() || self.parameter_hints.model_id.is_some_and(|id| Some(id) != model_id) {
            return self.cancel_parameter_hints(cx);
        }
        self.trigger_parameter_hints(CONTENT_CHANGE, None, DELAY, cx);
    }

    /// `_executeSignatureHelpProvider`: the providers' help at a position of a model, as an Invoke.
    fn execute_signature_help_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let uri = args.first().cloned().unwrap_or_default();
        let Some(editor) = super::host_requests::uri_parts(&uri).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("model not found")));
        };
        let document_uri = editor.document().uri();
        let candidate = Candidate { uri: &document_uri, language: &editor.document().language_id };
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect();
        let context = json!({ "triggerKind": INVOKE, "isRetrigger": false, "triggerCharacter": args.get(2).cloned().unwrap_or(Value::Null) });
        let request = self.provide_signature_help(handles, document_uri, args.get(1).cloned().unwrap_or_default(), context, cx);
        let (sender, receiver) = oneshot::channel();
        cx.background_spawn(async move {
            let _ = sender.send(Ok(request.await.unwrap_or(Value::Null)));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// The widget at the cursor, above the line when it fits, else below; at most 440px wide and a quarter of the
    /// editor (at least 250px) high.
    // shortcut: the docs do not scroll when taller than the widget and screen readers get no "hint" announcement.
    pub(super) fn parameter_hints_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hints = self.active_parameter_hints().filter(|_| self.parameter_hints.model_id == Some(editor.model_id))?;
        let signatures = signatures(hints);
        let active_signature = (hints["activeSignature"].as_u64().unwrap_or(0) as usize).min(signatures.len() - 1);
        let signature = &signatures[active_signature];
        let head = editor.selection().head;
        let text = &editor.document().lines[head.line];
        let left = visible_column(text, head.column, editor.document().indentation.tab_size) as f32 * char_width - editor.scroll.left + content_left;
        let anchor_top = editor.line_top(editor.view_line(head.line), self.editors.viewport) - editor.scroll.top;
        let editor_bounds = self.text_area_bounds.get();
        let window_size = window.viewport_size().map(f32::from);
        let measured = self.parameter_hints.widget_bounds.get().map(|bounds| Size { width: bounds.size.width + 2., height: bounds.size.height + 2. });
        let size = measured.unwrap_or(Size { width: 0., height: 0. });
        let left = horizontal_in_page(left, size.width, editor_bounds, window_size);
        // contentWidgets.ts: ABOVE when the box fits above the line in the window, else BELOW.
        let fits_above = editor_bounds.origin.y + anchor_top - size.height >= 0.;
        let top = if fits_above { anchor_top - size.height } else { anchor_top + LINE_HEIGHT };
        let max_height = (editor_bounds.size.height / 4.).max(250.);
        let high_contrast = self.theme.kind.is_high_contrast();
        let border = self.color("editorHoverWidget.border");
        let multiple = signatures.len() > 1;
        let parameter_index = signature["activeParameter"].as_u64().or(hints["activeParameter"].as_u64()).map(|index| index as usize);
        let parameters = signature["parameters"].as_array().map(Vec::as_slice).unwrap_or_default();
        let active_parameter = parameter_index.and_then(|index| parameters.get(index));
        let label = signature["label"].as_str().unwrap_or_default();
        let code = match parameters.is_empty() {
            true => div().child(label.to_string()),
            false => {
                let (start, end) = parameter_label_offsets(label, active_parameter);
                let highlight = self.color("editorHoverWidget.highlightForeground");
                div()
                    .flex()
                    .flex_wrap()
                    .child(label[..start].to_string())
                    .child(div().text_color(highlight).font_weight(FontWeight::BOLD).child(label[start..end].to_string()))
                    .child(label[end..].to_string())
            }
        };
        let style = MarkdownStyle { font_size: EDITOR_FONT_SIZE, line_height: LINE_HEIGHT, foreground: self.foreground_or("editorHoverWidget.foreground") };
        let language = &editor.document().language_id;
        let mut docs = Vec::new();
        if let Some(documentation) = active_parameter.map(|parameter| &parameter["documentation"]).filter(|documentation| has_text(documentation)) {
            docs.push(self.parameter_hints_docs("parameter-hints-parameter-docs", documentation, style, language, cx));
        }
        if has_text(&signature["documentation"]) {
            docs.push(self.parameter_hints_docs("parameter-hints-signature-docs", &signature["documentation"], style, language, cx));
        }
        let has_docs = !docs.is_empty();
        let signature_row = div()
            .px(px(5.))
            .py(px(4.))
            .font_family(EDITOR_FONT)
            .child(code)
            .when(has_docs, |row| row.border_b_1().border_color(border.opacity(0.5)));
        let body = div().flex_1().min_w_0().flex().flex_col().child(signature_row).when(has_docs, |body| body.child(div().pl(px(5.)).pr(px(10.)).children(docs)));
        let widget_bounds = self.parameter_hints.widget_bounds.clone();
        let measure = canvas(
            move |bounds, window, _| {
                let bounds: Bounds<f32> = bounds.map(f32::from);
                if widget_bounds.get().map(|old| old.size) != Some(bounds.size) {
                    window.refresh();
                }
                widget_bounds.set(Some(bounds));
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0();
        let digits = signatures.len().to_string().len();
        let overloads = format!("{:0digits$}/{}", active_signature + 1, signatures.len());
        let icon_color = self.foreground_or("editorHoverWidget.foreground");
        let controls = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .min_w(px(22.))
            .px(px(2.))
            .child(self.icon("chevron-up", 16., 16., icon_color).id("parameter-hints-previous").cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| {
                cx.stop_propagation();
                window.focus(&workbench.editor_focus, cx);
                workbench.cycle_parameter_hints(false, cx);
            })))
            .child(div().h(px(12.)).line_height(px(12.)).font_family(EDITOR_FONT).child(overloads))
            .child(self.icon("chevron-down", 16., 16., icon_color).id("parameter-hints-next").cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| {
                cx.stop_propagation();
                window.focus(&workbench.editor_focus, cx);
                workbench.cycle_parameter_hints(true, cx);
            })));
        let widget = div()
            .id("parameter-hints")
            .absolute()
            .left(px(left))
            .top(px(top))
            .max_w(px(440.))
            .max_h(px(max_height))
            .overflow_hidden()
            .flex()
            .when(multiple, |widget| widget.min_h(px(3.3 * 1.5 * EDITOR_FONT_SIZE)))
            .border(px(if high_contrast { 2. } else { 1. }))
            .border_color(border)
            .rounded(px(RADIUS))
            .bg(self.color("editorHoverWidget.background"))
            .text_color(self.foreground_or("editorHoverWidget.foreground"))
            .font_family(UI_FONT)
            .text_size(px(EDITOR_FONT_SIZE))
            .line_height(px(1.5 * EDITOR_FONT_SIZE))
            .cursor_default()
            .occlude()
            .when(measured.is_none(), |widget| widget.invisible())
            .when(!high_contrast, |widget| {
                widget.shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }])
            })
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(measure)
            .when(multiple, |widget| widget.child(controls).child(div().w(px(1.)).bg(border.opacity(0.5))))
            .child(body);
        Some(deferred(widget).with_priority(1).into_any_element())
    }

    fn parameter_hints_docs(&self, id: &str, documentation: &Value, style: MarkdownStyle, language: &str, cx: &mut Context<Self>) -> AnyElement {
        match documentation {
            Value::String(text) => div().my(px(8.)).whitespace_normal().child(text.clone()).into_any_element(),
            _ => {
                let blocks = parse(&MarkdownString::from_value(documentation));
                div().my(px(8.)).child(self.markdown_blocks(id, &blocks, style, language, cx)).into_any_element()
            }
        }
    }
}

fn signatures(hints: &Value) -> &[Value] {
    hints["signatures"].as_array().map(Vec::as_slice).unwrap_or_default()
}

/// `hasDocs`: a non-empty string or markdown value.
fn has_text(documentation: &Value) -> bool {
    match documentation {
        Value::String(text) => !text.is_empty(),
        Value::Object(markdown) => markdown.get("value").and_then(Value::as_str).is_some_and(|value| !value.is_empty()),
        _ => false,
    }
}

/// `getParameterLabelOffsets`: an offsets label as given (UTF-16 offsets into the signature label), else the first
/// match of the label as a whole word, else nothing.
fn parameter_label_offsets(label: &str, parameter: Option<&Value>) -> (usize, usize) {
    let Some(parameter) = parameter else { return (0, 0) };
    if let Some([start, end]) = parameter["label"].as_array().map(Vec::as_slice).and_then(|offsets| <&[Value; 2]>::try_from(offsets).ok()) {
        let byte = |utf16: u64| {
            let mut units = 0;
            for (index, character) in label.char_indices() {
                if units >= utf16 as usize {
                    return index;
                }
                units += character.len_utf16();
            }
            label.len()
        };
        let (start, end) = (byte(start.as_u64().unwrap_or(0)), byte(end.as_u64().unwrap_or(0)));
        return if start <= end { (start, end) } else { (0, 0) };
    }
    let name = parameter["label"].as_str().unwrap_or_default();
    if name.is_empty() {
        return (0, 0);
    }
    // `(\W|^)label(?=\W|$)`: `\W` is ASCII non-word in JavaScript.
    let is_word = |character: char| character.is_ascii_alphanumeric() || character == '_';
    for (index, _) in label.match_indices(name) {
        let before = label[..index].chars().last();
        let after = label[index + name.len()..].chars().next();
        if before.is_none_or(|character| !is_word(character)) && after.is_none_or(|character| !is_word(character)) {
            return (index, index + name.len());
        }
    }
    (0, 0)
}

#[cfg(test)]
mod tests;
