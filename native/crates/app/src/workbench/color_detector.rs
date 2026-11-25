//! colorDetector.ts, color.ts and defaultDocumentColorProvider.ts: the document colors of the active model from the
//! extension providers (or the worker's default ones), each drawn as a small color box injected before its range;
//! `_executeDocumentColorProvider` and `_executeColorPresentationProvider`.

use std::time::Duration;

use futures::channel::oneshot;
use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, Rgba, Task, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::default_document_colors::{compute_default_document_colors, default_color_presentations};
use crate::editor::injected_text::{InjectedText, InjectionOwner};
use crate::editor::{OpenEditor, Position, char_column, utf16_column};
use crate::language_features::Candidate;

const REGISTER: &str = "$registerDocumentColorProvider";
/// `ColorDetector.RECOMPUTE_TIME`, the debounce's minimum.
// shortcut: a fixed debounce, where `LanguageFeatureDebounceService` learns it from the providers' answer times.
const RECOMPUTE_TIME: Duration = Duration::from_millis(1000);
/// `colorDecoratorInnerWidthInEm` and `colorDecoratorMarginInEm`.
const INNER_WIDTH_EM: f32 = 0.8;
const MARGIN_EM: f32 = 0.2;
/// Menlo's advance in em, as inlay_hints.rs.
const EDITOR_CHAR_EM: f32 = 1233. / 2048.;
const EDITOR_FONT_SIZE: f32 = 12.;

#[derive(Default)]
pub(super) struct ColorDetector {
    model_id: Option<u32>,
    pub(super) colors: Vec<ColorData>,
    compute: Option<Task<()>>,
    /// The color picker the hover shows.
    pub(super) picker: Option<super::color_picker::HoverColorPicker>,
    pub(super) standalone: Option<super::standalone_color_picker::StandalonePicker>,
}

/// `IColorData`: a color, its range, and the provider that found it (`None`: the default provider).
#[derive(Clone, Debug)]
pub(super) struct ColorData {
    pub start: Position,
    pub end: Position,
    pub color: [f64; 4],
    pub handle: Option<u64>,
}

impl Workbench {
    pub(super) fn register_color_detector(&mut self) {
        self.commands.register("_executeDocumentColorProvider", |workbench, args, _, cx| workbench.execute_document_color_provider(&args, cx));
        self.commands.register("_executeColorPresentationProvider", |workbench, args, _, cx| workbench.execute_color_presentation_provider(&args, cx));
    }

    /// `onDidChangeModel`.
    pub(super) fn color_detector_model_changed(&mut self, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if model_id != self.color_detector.model_id {
            self.update_colors(cx);
        }
    }

    /// The registry or `editor.colorDecorators`, `editor.defaultColorDecorators`, `editor.colorDecoratorsLimit` changed.
    pub(super) fn color_providers_changed(&mut self, cx: &mut Context<Self>) {
        self.update_colors(cx);
    }

    /// `onDidChangeModelContent`: the colors are computed again `RECOMPUTE_TIME` after the first edit.
    pub(super) fn color_detector_content_changed(&mut self, cx: &mut Context<Self>) {
        if self.color_detector.model_id.is_none() || self.color_detector.compute.is_some() {
            return;
        }
        self.begin_compute_colors(RECOMPUTE_TIME, cx);
    }

    /// `updateColors`.
    fn update_colors(&mut self, cx: &mut Context<Self>) {
        self.remove_color_decorations();
        self.color_detector.compute = None;
        let Some(editor) = self.editors.active() else {
            self.color_detector.model_id = None;
            return;
        };
        self.color_detector.model_id = Some(editor.model_id);
        if !self.color_decorators_enabled(editor) {
            self.color_detector.model_id = None;
            return;
        }
        self.begin_compute_colors(Duration::ZERO, cx);
    }

    /// `isEnabled`: the deprecated `[languageId].colorDecorators.enable` false wins over `editor.colorDecorators`.
    fn color_decorators_enabled(&self, editor: &OpenEditor) -> bool {
        let language = editor.document().language_id.clone();
        if self.setting(&language)["colorDecorators"]["enable"] == json!(false) {
            return false;
        }
        self.suggest_setting(editor, "editor.colorDecorators") != json!(false)
    }

    fn begin_compute_colors(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(model_id) = self.color_detector.model_id else { return };
        self.color_detector.compute = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(pending) = workbench.update(cx, |workbench, cx| {
                let editor = workbench.editors.active().filter(|editor| editor.model_id == model_id)?;
                Some(workbench.find_color_data(editor, cx))
            }) else {
                return;
            };
            let Some(pending) = pending else { return };
            let colors = pending.await.unwrap_or_default();
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.color_detector.compute = None;
                if workbench.color_detector.model_id == Some(model_id) {
                    workbench.update_color_decorators(colors);
                    cx.notify();
                }
            });
        }));
    }

    /// `_findColorData` with `ColorDataCollector`: every extension provider in reverse score order; the default
    /// provider with `editor.defaultColorDecorators` `always`, with `auto` only when none of them answered an array.
    fn find_color_data(&self, editor: &OpenEditor, cx: &mut Context<Self>) -> oneshot::Receiver<Vec<ColorData>> {
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let calls: Vec<_> = self.language_features.ordered(REGISTER, &candidate).iter().rev().map(|registration| (registration.handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentColors", vec![json!(registration.handle), uri.clone()]))).collect();
        let enablement = self.suggest_setting(editor, "editor.defaultColorDecorators");
        let enablement = enablement.as_str().unwrap_or("auto").to_owned();
        let lines = editor.document().lines.clone();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let mut colors = Vec::new();
            let mut found = false;
            if enablement == "always" {
                colors.extend(default_colors(&lines));
            }
            for (handle, call) in calls {
                let Ok(Ok(Value::Array(infos))) = call.await else { continue };
                found = true;
                for info in infos {
                    colors.extend(color_data(&lines, &info, Some(handle)));
                }
            }
            if !found && enablement == "auto" {
                colors.extend(default_colors(&lines));
            }
            let _ = sender.send(colors);
        })
        .detach();
        receiver
    }

    /// `updateColorDecorators`: up to `editor.colorDecoratorsLimit` boxes, each `0.8em` plus `0.2em` margins wide.
    fn update_color_decorators(&mut self, colors: Vec<ColorData>) {
        let Some(model_id) = self.color_detector.model_id else { return };
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == model_id) else { return };
        let limit = self.suggest_setting(editor, "editor.colorDecoratorsLimit").as_u64().unwrap_or(500) as usize;
        let columns = (INNER_WIDTH_EM + 2. * MARGIN_EM) / EDITOR_CHAR_EM;
        let injections = colors.iter().take(limit).enumerate().map(|(id, color)| InjectedText { position: color.start, columns, owner: InjectionOwner::Color, id, at_range_start: true }).collect();
        self.color_detector.colors = colors;
        let editor = self.editors.active_mut().expect("checked above");
        editor.set_injected_text(InjectionOwner::Color, injections);
    }

    fn remove_color_decorations(&mut self) {
        self.color_detector.colors.clear();
        if let Some(model_id) = self.color_detector.model_id
            && let Some(editor) = self.editors.editors.iter_mut().chain(&mut self.editors.models).find(|editor| editor.model_id == model_id)
        {
            editor.set_injected_text(InjectionOwner::Color, Vec::new());
        }
    }

    /// colorPicker.css `.colorpicker-color-decoration`: the color in a 0.8em box with a 0.1em border (`#eee` on dark
    /// and high contrast themes, `#000` on light ones), 0.2em margins, the pointer cursor; a click shows the picker.
    pub(super) fn color_decorator_element(&self, id: usize, left: f32, line_height: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let data = self.color_detector.colors.get(id)?;
        let em = EDITOR_FONT_SIZE * self.editor_zoom.multiplier();
        let border = match self.theme.kind.is_light() {
            true => gpui::rgb(0x000000),
            false => gpui::rgb(0xeeeeee),
        };
        let [r, g, b, a] = data.color;
        let byte = |value: f64| ((value * 255.).round() / 255.) as f32;
        let fill = Rgba { r: byte(r), g: byte(g), b: byte(b), a: a as f32 };
        let size = INNER_WIDTH_EM * em;
        let start = data.start;
        Some(
            div()
                .id(("color-decoration", id))
                .absolute()
                .left(px(left + MARGIN_EM * em))
                // `margin-top: 0.1em` on an inline-block in the line box.
                .top(px(((line_height - size) / 2.).round()))
                .size(px(size))
                .bg(fill)
                .border(px(0.1 * em))
                .border_color(border)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.color_decoration_clicked(start, window, cx);
                }))
                .into_any_element(),
        )
    }

    /// `ColorDetector.getColorData`: the color whose range holds `position`.
    pub(super) fn color_data_at(&self, position: Position) -> Option<&ColorData> {
        self.color_detector.colors.iter().find(|color| color.start <= position && position <= color.end)
    }

    /// `_executeDocumentColorProvider`: `{ range, color: [r, g, b, a] }` of every color of a model.
    fn execute_document_color_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let lines = editor.document().lines.clone();
        let pending = self.find_color_data(editor, cx);
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let colors = pending.await.unwrap_or_default();
            let infos: Vec<Value> = colors.iter().map(|color| json!({ "range": range_dto(&lines, color.start, color.end), "color": color.color })).collect();
            let _ = sender.send(Ok(Value::Array(infos)));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_executeColorPresentationProvider`: the presentations of `[r, g, b, a]` at `{ uri, range }`, from the
    /// extension providers, else the default provider's.
    fn execute_color_presentation_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let (color, context) = (args.first().cloned().unwrap_or_default(), args.get(1).cloned().unwrap_or_default());
        if context.is_null() {
            return Outcome::done();
        }
        let channels: Option<Vec<f64>> = color.as_array().map(|channels| channels.iter().filter_map(Value::as_f64).collect());
        let editor = super::host_requests::uri_parts(&context["uri"]).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path));
        let (Some(channels), Some(editor)) = (channels.filter(|channels| channels.len() == 4), editor) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let color = [channels[0], channels[1], channels[2], channels[3]];
        Outcome::Pending(self.color_presentations(editor, color, context["range"].clone(), cx))
    }

    /// `_findColorData` with `ColorPresentationsCollector`.
    pub(super) fn color_presentations(&self, editor: &OpenEditor, color: [f64; 4], range: Value, cx: &mut Context<Self>) -> oneshot::Receiver<Result<Value, Value>> {
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        let info = json!({ "color": color, "range": range });
        let calls: Vec<_> = self.language_features.ordered(REGISTER, &candidate).iter().rev().map(|registration| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideColorPresentations", vec![json!(registration.handle), uri.clone(), info.clone()])).collect();
        let enablement = self.suggest_setting(editor, "editor.defaultColorDecorators");
        let enablement = enablement.as_str().unwrap_or("auto").to_owned();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let mut presentations = Vec::new();
            let mut found = false;
            for call in calls {
                if let Ok(Ok(Value::Array(list))) = call.await {
                    found = true;
                    presentations.extend(list);
                }
            }
            if (!found && enablement != "never") || enablement == "always" {
                presentations.extend(default_color_presentations(color).into_iter().map(|label| json!({ "label": label, "textEdit": { "range": range, "text": label } })));
            }
            let _ = sender.send(Ok(Value::Array(presentations)));
        })
        .detach();
        receiver
    }
}

fn default_colors(lines: &[String]) -> Vec<ColorData> {
    compute_default_document_colors(lines).into_iter().map(|info| ColorData { start: info.start, end: info.end, color: info.color, handle: None }).collect()
}

/// A provider's `{ color: [r, g, b, a], range }` on the model's lines.
fn color_data(lines: &[String], info: &Value, handle: Option<u64>) -> Option<ColorData> {
    let channels: Vec<f64> = info["color"].as_array()?.iter().filter_map(Value::as_f64).collect();
    let [r, g, b, a] = channels.as_slice() else { return None };
    let position = |line: &Value, column: &Value| {
        let line = (line.as_u64().unwrap_or(1).max(1) as usize - 1).min(lines.len() - 1);
        Position::new(line, char_column(&lines[line], column.as_u64().unwrap_or(1).max(1) as usize - 1))
    };
    let range = &info["range"];
    Some(ColorData { start: position(&range["startLineNumber"], &range["startColumn"]), end: position(&range["endLineNumber"], &range["endColumn"]), color: [*r, *g, *b, *a], handle })
}

pub(super) fn range_dto(lines: &[String], start: Position, end: Position) -> Value {
    json!({ "startLineNumber": start.line + 1, "startColumn": utf16_column(&lines[start.line], start.column) + 1, "endLineNumber": end.line + 1, "endColumn": utf16_column(&lines[end.line], end.column) + 1 })
}
