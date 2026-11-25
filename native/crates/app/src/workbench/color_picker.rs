//! colorPickerModel.ts, colorPickerWidget.ts and its parts (header, saturation box, hue and opacity strips),
//! colorPickerParticipantUtils.ts and hoverColorPickerParticipant.ts: the picker the hover shows over a document
//! color; a change of the color asks the color's provider for its presentations, a flush writes the presentation
//! into the document.

use std::cell::Cell;
use std::rc::Rc;

use futures::channel::oneshot;
use gpui::{AnyElement, Bounds, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Rgba, canvas, div, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba};
use serde_json::{Value, json};

use super::Workbench;
use super::color_detector::{ColorData, range_dto};
use crate::default_document_colors::default_color_presentations;
use crate::editor::{EditReason, OpenEditor, Position, char_column, text_between};

/// colorPicker.css sizes.
const HEADER_HEIGHT: f32 = 24.;
const BODY_PADDING: f32 = 8.;
const BOX_HEIGHT: f32 = 150.;
const SATURATION_MIN_WIDTH: f32 = 220.;
const STRIP_WIDTH: f32 = 25.;
const STRIP_GAP: f32 = 8.;
const ORIGINAL_WIDTH: f32 = 74.;
const SLIDER_HEIGHT: f32 = 4.;
/// `.standalone-strip`: shorter, leaving room for the Insert button under it.
const STANDALONE_STRIP_HEIGHT: f32 = 122.;
/// standaloneColorPickerWidget.ts `CLOSE_BUTTON_WIDTH`.
const CLOSE_BUTTON_WIDTH: f32 = 22.;
/// `.colorpicker-widget` height.
pub(super) const STANDALONE_HEIGHT: f32 = 190.;
/// The header's `.picked-color` (240px) and `.original-color` (74px), wider than the hover's 302px minimum.
pub(super) const PICKER_WIDTH: f32 = 314.;

/// `Color` with the `HSVA` it was made from (the picker keeps its hue when the saturation or value is 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PickerColor {
    pub rgb: [u8; 3],
    pub alpha: f64,
    pub hsva: [f64; 4],
}

impl PickerColor {
    /// `new Color(new HSVA(h, s, v, a))`.
    pub fn from_hsva(h: f64, s: f64, v: f64, a: f64) -> Self {
        let round3 = |value: f64| (value.clamp(0., 1.) * 1000.).round() / 1000.;
        let (h, s, v, a) = (h.clamp(0., 360.).trunc(), round3(s), round3(v), round3(a));
        let c = v * s;
        let x = c * (1. - ((h / 60.) % 2. - 1.).abs());
        let m = v - c;
        let (r, g, b) = match h {
            _ if h < 60. => (c, x, 0.),
            _ if h < 120. => (x, c, 0.),
            _ if h < 180. => (0., c, x),
            _ if h < 240. => (0., x, c),
            _ if h < 300. => (x, 0., c),
            _ => (c, 0., x),
        };
        let byte = |value: f64| ((value + m) * 255.).round().clamp(0., 255.) as u8;
        PickerColor { rgb: [byte(r), byte(g), byte(b)], alpha: a, hsva: [h, s, v, a] }
    }

    /// `new Color(new RGBA(r, g, b, a))` with `HSVA.fromRGBA`.
    pub fn from_rgba(rgb: [u8; 3], a: f64) -> Self {
        let a = (a.clamp(0., 1.) * 1000.).round() / 1000.;
        let [r, g, b] = rgb.map(|channel| channel as f64 / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let delta = max - min;
        let s = if max == 0. { 0. } else { delta / max };
        let m = match delta {
            0. => 0.,
            _ if max == r => (((g - b) / delta % 6.) + 6.) % 6.,
            _ if max == g => (b - r) / delta + 2.,
            _ => (r - g) / delta + 4.,
        };
        let round3 = |value: f64| (value * 1000.).round() / 1000.;
        PickerColor { rgb, alpha: a, hsva: [(m * 60.).round(), round3(s), round3(max), a] }
    }

    /// The `IColor` channels from 0 to 1.
    pub fn channels(&self) -> [f64; 4] {
        [self.rgb[0] as f64 / 255., self.rgb[1] as f64 / 255., self.rgb[2] as f64 / 255., self.alpha]
    }

    fn fill(&self) -> Rgba {
        Rgba { r: self.rgb[0] as f32 / 255., g: self.rgb[1] as f32 / 255., b: self.rgb[2] as f32 / 255., a: self.alpha as f32 }
    }

    /// `Color.isLighter`: YIQ brightness of 128 or more.
    fn is_lighter(&self) -> bool {
        (self.rgb[0] as f64 * 299. + self.rgb[1] as f64 * 587. + self.rgb[2] as f64 * 114.) / 1000. >= 128.
    }
}

/// What the hover's picker edits: `ColorPickerModel` and the range its presentation replaces.
pub(super) struct HoverColorPicker {
    pub model_id: u32,
    pub start: Position,
    pub end: Position,
    pub handle: Option<u64>,
    pub original: PickerColor,
    pub color: PickerColor,
    pub presentations: Vec<Value>,
    pub index: usize,
    /// The part under a drag: the saturation box, the opacity strip or the hue strip.
    drag: Option<DragPart>,
    /// `editorUpdatedByColorPicker`: the next content change is the picker's own.
    pub updated_by_picker: bool,
    /// `ColorPickerWidgetType.Standalone`: a flush keeps the color for Insert instead of editing.
    pub standalone: bool,
    bounds: Rc<[Cell<Option<Bounds<f32>>>; 3]>,
}

#[derive(Clone, Copy, PartialEq)]
enum DragPart {
    Saturation,
    Opacity,
    Hue,
}

impl HoverColorPicker {
    fn presentation_label(&self) -> &str {
        self.presentations.get(self.index).and_then(|presentation| presentation["label"].as_str()).unwrap_or_default()
    }

    /// `guessColorPresentation`: the presentation whose label is the original text, else the first whose label
    /// starts with the text before its `(`.
    fn guess_presentation(&mut self, original_text: &str) {
        let original = original_text.to_lowercase();
        let labels: Vec<String> = self.presentations.iter().map(|presentation| presentation["label"].as_str().unwrap_or_default().to_owned()).collect();
        let index = labels.iter().position(|label| *label == original).or_else(|| {
            let prefix = original.split('(').next().unwrap_or_default();
            labels.iter().position(|label| label.to_lowercase().starts_with(prefix))
        });
        if let Some(index) = index {
            self.index = index;
        }
    }

    /// The `colorPresentations` setter: an index past the new list goes back to the first.
    fn set_presentations(&mut self, presentations: Vec<Value>) {
        self.presentations = presentations;
        if self.index >= self.presentations.len() {
            self.index = 0;
        }
    }
}

impl Workbench {
    /// hoverColorPickerContribution.ts `onMouseDown` on a decorator with `editor.colorDecoratorsActivatedOn` `click`
    /// or `clickAndHover`: the hover shows at once for the color.
    pub(super) fn color_decoration_clicked(&mut self, start: Position, _window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let activated_on = self.suggest_setting(editor, "editor.colorDecoratorsActivatedOn");
        if !matches!(activated_on.as_str().unwrap_or("clickAndHover"), "click" | "clickAndHover") {
            return;
        }
        self.show_content_hover_at(start, super::content_hover::HoverSource::Click, cx);
    }

    /// `_isValidRequest`: a mouse hover with `hover` or `clickAndHover`, a click with `click` or `clickAndHover`,
    /// the keyboard always.
    pub(super) fn color_hover_allowed(&self, editor: &OpenEditor, source: super::content_hover::HoverSource) -> bool {
        let activated_on = self.suggest_setting(editor, "editor.colorDecoratorsActivatedOn");
        let activated_on = activated_on.as_str().unwrap_or("clickAndHover");
        match source {
            super::content_hover::HoverSource::Mouse => matches!(activated_on, "hover" | "clickAndHover"),
            super::content_hover::HoverSource::Click => matches!(activated_on, "click" | "clickAndHover"),
            super::content_hover::HoverSource::Keyboard => true,
        }
    }

    /// `createColorHover`: the color's presentations from its provider, the model guessing the one the text uses.
    pub(super) fn create_color_hover(&self, editor: &OpenEditor, data: &ColorData, cx: &mut Context<Self>) -> oneshot::Receiver<HoverColorPicker> {
        let color = PickerColor::from_rgba(data.color.map(|channel| (channel * 255.).round().clamp(0., 255.) as u8)[..3].try_into().expect("three channels"), data.color[3]);
        let original_text = text_between(&editor.document().lines, data.start, data.end);
        let pending = self.provider_color_presentations(editor, data.handle, color.channels(), data.start, data.end, cx);
        let (sender, receiver) = oneshot::channel();
        let (model_id, start, end, handle) = (editor.model_id, data.start, data.end, data.handle);
        cx.spawn(async move |_, _| {
            let presentations = pending.await.unwrap_or_default();
            let mut picker = HoverColorPicker { model_id, start, end, handle, original: color, color, presentations: Vec::new(), index: 0, drag: None, updated_by_picker: false, standalone: false, bounds: Rc::default() };
            picker.set_presentations(presentations);
            picker.guess_presentation(&original_text);
            let _ = sender.send(picker);
        })
        .detach();
        receiver
    }

    /// `getColorPresentations` of one provider; the default provider's labels without a call.
    fn provider_color_presentations(&self, editor: &OpenEditor, handle: Option<u64>, color: [f64; 4], start: Position, end: Position, cx: &mut Context<Self>) -> oneshot::Receiver<Vec<Value>> {
        let (sender, receiver) = oneshot::channel();
        let range = range_dto(&editor.document().lines, start, end);
        let Some(handle) = handle else {
            let presentations = default_color_presentations(color).into_iter().map(|label| json!({ "label": label })).collect();
            let _ = sender.send(presentations);
            return receiver;
        };
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", "$provideColorPresentations", vec![json!(handle), editor.document().uri(), json!({ "color": color, "range": range })]);
        cx.spawn(async move |_, _| {
            let presentations = match call.await {
                Ok(Ok(Value::Array(list))) => list,
                _ => Vec::new(),
            };
            let _ = sender.send(presentations);
        })
        .detach();
        receiver
    }

    /// The standalone picker while it shows, else the hover's.
    fn picker(&self) -> Option<&HoverColorPicker> {
        match &self.color_detector.standalone {
            Some(standalone) => Some(&standalone.picker),
            None => self.color_detector.picker.as_ref(),
        }
    }

    fn picker_mut(&mut self) -> Option<&mut HoverColorPicker> {
        match &mut self.color_detector.standalone {
            Some(standalone) => Some(&mut standalone.picker),
            None => self.color_detector.picker.as_mut(),
        }
    }

    /// `model.color = ...` then `onDidChangeColor`: the presentations follow the new color.
    fn set_picker_color(&mut self, color: PickerColor, cx: &mut Context<Self>) {
        let Some(picker) = self.picker_mut() else { return };
        if picker.color == color {
            return;
        }
        picker.color = color;
        self.update_picker_presentations(None, cx);
        cx.notify();
    }

    /// `updateColorPresentations`, and with `flush` after it `updateEditorModel`.
    fn update_picker_presentations(&mut self, flush: Option<()>, cx: &mut Context<Self>) {
        let Some(picker) = self.picker() else { return };
        let Some(editor) = self.editors.active().filter(|editor| editor.model_id == picker.model_id) else { return };
        let pending = self.provider_color_presentations(editor, picker.handle, picker.color.channels(), picker.start, picker.end, cx);
        let color = picker.color;
        cx.spawn(async move |workbench, cx| {
            let presentations = pending.await.unwrap_or_default();
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(picker) = workbench.picker_mut().filter(|picker| flush.is_some() || picker.color == color) else { return };
                picker.set_presentations(presentations);
                if flush.is_some() && !picker.standalone {
                    workbench.apply_picker_presentation(None, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `onColorFlushed`.
    fn flush_picker_color(&mut self, cx: &mut Context<Self>) {
        self.update_picker_presentations(Some(()), cx);
    }

    /// `updateEditorModel`: the presentation's text edit (or its label over the range) and its additional edits,
    /// one undo stop; the range follows the replaced text (`GrowsOnlyWhenTypingAfter`). With `insertion_ranges`
    /// (the standalone picker's Insert) the text goes at each of them instead.
    pub(super) fn apply_picker_presentation(&mut self, insertion_ranges: Option<Vec<(Position, Position)>>, cx: &mut Context<Self>) {
        let Some(picker) = self.picker() else { return };
        let Some(presentation) = picker.presentations.get(picker.index).cloned() else { return };
        let (model_id, start, end) = (picker.model_id, picker.start, picker.end);
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.model_id == model_id) else { return };
        let lines = editor.document().lines.clone();
        let position = |line: &Value, column: &Value| {
            let line = (line.as_u64().unwrap_or(1).max(1) as usize - 1).min(lines.len() - 1);
            Position::new(line, char_column(&lines[line], column.as_u64().unwrap_or(1).max(1) as usize - 1))
        };
        let edit_of = |edit: &Value| {
            let range = &edit["range"];
            (position(&range["startLineNumber"], &range["startColumn"]), position(&range["endLineNumber"], &range["endColumn"]), edit["text"].as_str().unwrap_or_default().to_owned(), false)
        };
        let mut edits: Vec<_> = presentation["additionalTextEdits"].as_array().into_iter().flatten().map(edit_of).collect();
        let main = match presentation["textEdit"].is_object() {
            true => edit_of(&presentation["textEdit"]),
            false => (start, end, presentation["label"].as_str().unwrap_or_default().to_owned(), false),
        };
        let (replace_start, text) = (main.0, main.2.clone());
        match insertion_ranges {
            Some(ranges) => edits.extend(ranges.into_iter().map(|(start, end)| (start, end, text.clone(), false))),
            None => edits.push(main),
        }
        let change = editor.execute_edits(edits, None, EditReason::Unknown("colorpicker"));
        editor.push_undo_stop();
        let new_end = crate::editor::end_of(replace_start, &text);
        if let Some(picker) = self.picker_mut() {
            picker.start = replace_start;
            picker.end = new_end;
            picker.updated_by_picker = change.is_some();
        }
        self.editor_edited(change, true, "api", cx);
    }

    /// The picker's `onDidChangeModelContent`: an edit not made by the picker hides the hover.
    pub(super) fn color_picker_content_changed(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker_mut() else { return };
        if picker.updated_by_picker {
            picker.updated_by_picker = false;
            return;
        }
        match picker.standalone {
            true => self.hide_standalone_color_picker(cx),
            false => self.hide_content_hover(cx),
        }
    }

    /// The hover's picker row: `ColorPickerWidget` of `ColorPickerWidgetType.Hover`.
    pub(super) fn color_picker_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(picker) = self.picker() else { return div().into_any_element() };
        let standalone = picker.standalone;
        let strip_height = if standalone { STANDALONE_STRIP_HEIGHT } else { BOX_HEIGHT };
        let color = picker.color;
        let background = self.color("editorHoverWidget.background");
        let background_lighter = (background.r * 299. + background.g * 587. + background.b * 114.) * 255. / 1000. >= 128.;
        let light = if color.alpha < 0.5 { background_lighter } else { color.is_lighter() };
        let original = picker.original;
        let header = div()
            .flex()
            .h(px(HEADER_HEIGHT))
            // The standalone picker sits in the editor's element, which sets the editor font.
            .font_family(".SystemUIFont")
            // roundedCorners.css `.modern-ui .colorpicker-widget`: the widget clips its header to the hover's corners.
            .when(!standalone, |header| header.rounded_t(px(super::content_hover_view::RADIUS - 1.)).overflow_hidden())
            .relative()
            .child(checkerboard())
            .child(
                div()
                    .id("picked-color")
                    .relative()
                    .when(!standalone, |picked| picked.flex_1().min_w(px(240.)))
                    .when(standalone, |picked| picked.w(px(self.saturation_width() + BODY_PADDING)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .bg(color.fill())
                    .text_color(if light { rgb(0x000000) } else { rgb(0xffffff) })
                    .child(self.icon("color-mode", 14., 14., if light { rgb(0x000000).into() } else { rgb(0xffffff).into() }))
                    .child(div().mx(px(5.)).child(picker.presentation_label().to_owned()))
                    .child(self.hover_target("picked-color-title", super::hover::HoverContent::Text("Click to toggle color options (rgb/hsl/hex)".into()), super::hover::HoverOptions::mouse()))
                    .on_click(cx.listener(|workbench, _, _, cx| workbench.select_next_color_presentation(cx))),
            )
            .child(
                div()
                    .id("original-color")
                    .relative()
                    .when(!standalone, |original| original.w(px(ORIGINAL_WIDTH)))
                    .when(standalone, |original| original.flex_1())
                    .cursor_pointer()
                    .bg(original.fill())
                    .on_click(cx.listener(move |workbench, _, _, cx| {
                        workbench.set_picker_color(original, cx);
                        workbench.flush_picker_color(cx);
                    })),
            )
            .when(standalone, |header| header.child(self.color_picker_close_button(cx)));
        let [hue, saturation, value, alpha] = color.hsva;
        let bounds = picker.bounds.clone();
        let measure = |index: usize| {
            let bounds = bounds.clone();
            canvas(move |element_bounds, _, _| bounds[index].set(Some(element_bounds.map(f32::from))), |_, _, _, _| ()).absolute().inset_0()
        };
        let saturated = PickerColor::from_hsva(hue, 1., 1., 1.).fill();
        let saturation_box = div()
            .id("saturation-box")
            .relative()
            .flex_1()
            .min_w(px(SATURATION_MIN_WIDTH))
            .h(px(BOX_HEIGHT))
            .overflow_hidden()
            .bg(saturated)
            .child(measure(0))
            .child(div().absolute().inset_0().bg(linear_gradient(90., linear_color_stop(rgba(0xffffffff), 0.), linear_color_stop(rgba(0xffffff00), 1.))))
            .child(div().absolute().inset_0().bg(linear_gradient(180., linear_color_stop(rgba(0x00000000), 0.), linear_color_stop(rgba(0x000000ff), 1.))))
            .child(self.saturation_selection(saturation, value))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, _, cx| workbench.picker_drag_start(DragPart::Saturation, event.position.map(f32::from), cx)));
        let fill = color.fill();
        let opaque = Rgba { a: 1., ..fill };
        let transparent = Rgba { a: 0., ..fill };
        let opacity_strip = div()
            .id("opacity-strip")
            .relative()
            .ml(px(STRIP_GAP))
            .w(px(STRIP_WIDTH))
            .h(px(strip_height))
            .cursor_grab()
            .child(checkerboard())
            .child(measure(1))
            .child(div().absolute().inset_0().bg(linear_gradient(180., linear_color_stop(opaque, 0.), linear_color_stop(transparent, 1.))))
            .child(slider(1. - alpha, strip_height))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, _, cx| workbench.picker_drag_start(DragPart::Opacity, event.position.map(f32::from), cx)));
        let hue_strip = div()
            .id("hue-strip")
            .relative()
            .ml(px(STRIP_GAP))
            .w(px(STRIP_WIDTH))
            .h(px(strip_height))
            .cursor_grab()
            .flex()
            .flex_col()
            .children(hue_segments(strip_height))
            .child(measure(2))
            .child(slider(hue / 360., strip_height))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, _, cx| workbench.picker_drag_start(DragPart::Hue, event.position.map(f32::from), cx)));
        let found_in_editor = self.color_detector.standalone.as_ref().is_some_and(|standalone| standalone.found_in_editor);
        let body = div()
            .flex()
            .items_start()
            .p(px(BODY_PADDING))
            .relative()
            .child(saturation_box)
            .child(opacity_strip)
            .child(hue_strip)
            .when(standalone, |body| body.border_b_1().border_color(self.color("editorHoverWidget.border")).child(self.color_picker_insert_button(found_in_editor, cx)));
        div()
            .id("color-picker")
            .w(px(PICKER_WIDTH))
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .on_mouse_move(cx.listener(|workbench, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    workbench.picker_drag_move(event.position.map(f32::from), cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|workbench, _: &MouseUpEvent, _, cx| workbench.picker_drag_end(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|workbench, _: &MouseUpEvent, _, cx| workbench.picker_drag_end(cx)))
            .into_any_element()
    }

    fn saturation_width(&self) -> f32 {
        self.picker().and_then(|picker| picker.bounds[0].get()).map_or(SATURATION_MIN_WIDTH, |bounds| bounds.size.width)
    }

    /// `.close-button`: a 22px column with the `close` codicon at the header's end, `toolbar.hoverBackground` on hover.
    fn color_picker_close_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.color("toolbar.hoverBackground");
        div()
            .id("color-picker-close")
            .w(px(CLOSE_BUTTON_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(self.color("editorHoverWidget.background"))
            .border_l_1()
            .border_color(self.color("editorHoverWidget.border"))
            .hover(move |button| button.bg(hover))
            .child(self.icon("close", 16., 16., self.foreground_or("icon.foreground")))
            .on_click(cx.listener(|workbench, _, _, cx| workbench.hide_standalone_color_picker(cx)))
    }

    /// `.insert-button`: Insert, or Replace on a document color, at the body's bottom right.
    fn color_picker_insert_button(&self, found_in_editor: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.color("button.hoverBackground");
        div()
            .id("color-picker-insert")
            .absolute()
            .right(px(BODY_PADDING))
            .bottom(px(BODY_PADDING))
            .w(px(58.))
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(2.))
            .cursor_pointer()
            .bg(self.color("button.background"))
            .text_color(self.color("button.foreground"))
            .hover(move |button| button.bg(hover))
            .child(if found_in_editor { "Replace" } else { "Insert" })
            .on_click(cx.listener(|workbench, _, _, cx| workbench.insert_standalone_color(cx)))
    }

    /// `.saturation-selection`: a 9px white ring with a dark glow at the saturation and value.
    fn saturation_selection(&self, saturation: f64, value: f64) -> Div {
        let width = self.saturation_width();
        div()
            .absolute()
            .left(px(saturation as f32 * width - 5.))
            .top(px(BOX_HEIGHT - value as f32 * BOX_HEIGHT - 5.))
            .size(px(9.))
            .rounded_full()
            .border_1()
            .border_color(rgb(0xffffff))
            .shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.8), offset: gpui::point(px(0.), px(0.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
    }

    /// `selectNextColorPresentation` then `flushColor`.
    fn select_next_color_presentation(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker_mut() else { return };
        if picker.presentations.is_empty() {
            return;
        }
        picker.index = (picker.index + 1) % picker.presentations.len();
        self.flush_picker_color(cx);
        cx.notify();
    }

    fn picker_drag_start(&mut self, part: DragPart, position: gpui::Point<f32>, cx: &mut Context<Self>) {
        let Some(picker) = self.picker_mut() else { return };
        picker.drag = Some(part);
        cx.stop_propagation();
        self.picker_drag_move(position, cx);
    }

    /// `onDidChangePosition` of the saturation box and `onDidChangeTop` of the strips.
    fn picker_drag_move(&mut self, position: gpui::Point<f32>, cx: &mut Context<Self>) {
        let Some(picker) = self.picker() else { return };
        let Some(part) = picker.drag else { return };
        let index = match part {
            DragPart::Saturation => 0,
            DragPart::Opacity => 1,
            DragPart::Hue => 2,
        };
        let Some(bounds) = picker.bounds[index].get() else { return };
        let [h, s, v, a] = picker.color.hsva;
        let top = position.y - bounds.origin.y;
        let color = match part {
            DragPart::Saturation => {
                let saturation = ((position.x - bounds.origin.x) / bounds.size.width).clamp(0., 1.) as f64;
                let value = (1. - top / bounds.size.height).clamp(0., 1.) as f64;
                PickerColor::from_hsva(h, saturation, value, a)
            }
            DragPart::Opacity | DragPart::Hue => {
                let value = (1. - top / bounds.size.height).clamp(0., 1.) as f64;
                match part {
                    DragPart::Opacity => PickerColor::from_hsva(h, s, v, value),
                    _ => {
                        let hue = (1. - value) * 360.;
                        PickerColor::from_hsva(if hue == 360. { 0. } else { hue }, s, v, a)
                    }
                }
            }
        };
        self.set_picker_color(color, cx);
    }

    /// The pointer up after a drag flushes the color.
    fn picker_drag_end(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker_mut() else { return };
        if picker.drag.take().is_some() {
            self.flush_picker_color(cx);
        }
    }
}

/// `.slider`: 4px tall, 2px past each side of the strip, a light border with a dark glow, at `(1 - value)` of the
/// strip's height less its own.
fn slider(position: f64, strip_height: f32) -> Div {
    div()
        .absolute()
        .left(px(-2.))
        .w(px(STRIP_WIDTH + 4.))
        .top(px(position as f32 * (strip_height - SLIDER_HEIGHT)))
        .h(px(SLIDER_HEIGHT))
        .border_1()
        .border_color(rgba(0xffffffb5))
        .shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.85), offset: gpui::point(px(0.), px(0.)), blur_radius: px(1.), spread_radius: px(0.), inset: false }])
}

/// The hue strip's `linear-gradient(to bottom, red 0%, yellow 17%, lime 33%, cyan 50%, blue 67%, magenta 83%, red 100%)`
/// as one two-stop gradient per segment (GPUI gradients have two stops).
fn hue_segments(strip_height: f32) -> Vec<Div> {
    let stops = [(0xff0000ff_u32, 0.), (0xffff00ff, 0.17), (0x00ff00ff, 0.33), (0x00ffffff, 0.5), (0x0000ffff, 0.67), (0xff00ffff, 0.83), (0xff0000ff, 1.)];
    stops
        .windows(2)
        .map(|pair| {
            let ((from, start), (to, end)) = (pair[0], pair[1]);
            div().w_full().h(px((end - start) * strip_height)).bg(linear_gradient(180., linear_color_stop(rgba(from), 0.), linear_color_stop(rgba(to), 1.)))
        })
        .collect()
}

/// `url('images/opacity-background.png')` at `9px 9px`: 4.5px squares of `#BBBBBB` and `#444444` at 10% alpha.
fn checkerboard() -> AnyElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let square: f32 = 4.5;
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let mut y: f32 = 0.;
            let mut row = 0;
            while y < height {
                let mut x: f32 = 0.;
                let mut column = 0;
                while x < width {
                    let origin = bounds.origin + gpui::point(px(x), px(y));
                    let size = gpui::size(px(square.min(width - x)), px(square.min(height - y)));
                    let color = if (row + column) % 2 == 0 { rgba(0xbbbbbb19) } else { rgba(0x44444419) };
                    window.paint_quad(gpui::fill(Bounds { origin, size }, color));
                    x += square;
                    column += 1;
                }
                y += square;
                row += 1;
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

#[cfg(test)]
mod tests;
