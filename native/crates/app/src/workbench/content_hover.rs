//! contentHoverController.ts, contentHoverWidgetWrapper.ts and hoverOperation.ts: the hover over the editor's
//! text, from the problem markers and unicode highlights under the mouse and the extensions' hover providers.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::oneshot;
use config::configuration::Overrides;
use gpui::{Bounds, Context, Modifiers, Pixels, Point, Task};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::unicode_highlighter::{CodePointHighlighter, HighlightReason, not_ambiguous_in_locales};
use crate::editor::{OpenEditor, Position, utf16_column, visible_column};
use crate::language_features::Candidate;
use crate::markers::Marker;

#[derive(Default)]
pub(super) struct ContentHover {
    /// `HoverOperation` with the computer's anchor; it stays after it completes, until the hover hides.
    operation: Option<Operation>,
    /// `_currentResult`: what the widget shows.
    pub(super) shown: Option<Shown>,
    /// `_reactToEditorMouseMoveRunner` and the `_mouseMoveEvent` it reacts to.
    reschedule: Option<Task<()>>,
    last_move: Option<MouseMove>,
    /// The model line and visible column under the mouse, set by the line's row before the text area reacts.
    pub(super) row_target: Option<(usize, f32)>,
    /// The widget's window bounds, once painted.
    pub(super) widget_bounds: Rc<Cell<Option<Bounds<f32>>>>,
    /// The rows' height before `max_height` clips them, which `_findPositionPreference` compares to the space.
    pub(super) content_height: Rc<Cell<f32>>,
    serial: usize,
    /// Counts the shows from hidden, which replay the `fade-in` animation.
    pub(super) shows: usize,
}

/// `HoverRangeAnchor` on one model line: the chars `start..end`, or the empty range at a position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Anchor {
    pub model_id: u32,
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy)]
pub(super) enum MouseTarget {
    /// The text, or the empty content of a line, at a visible column.
    Text { line: usize, visible: f32 },
    Widget,
    Other,
}

#[derive(Clone, Copy)]
struct MouseMove {
    target: MouseTarget,
    position: Point<f32>,
    modifiers: Modifiers,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Delayed,
    Immediate,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    FirstWait,
    SecondWait,
    WaitingForAsync,
    WaitingForAsyncShowingLoading,
    Idle,
}

struct Operation {
    anchor: Anchor,
    keyboard: bool,
    insist: bool,
    initial_mouse: Option<Point<f32>>,
    state: State,
    pending_providers: usize,
    parts: Vec<Part>,
    serial: usize,
    _tasks: Vec<Task<()>>,
}

/// The participants in `hoverOrdinal` order: the widget renders their parts in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Participant {
    Marker,
    Markdown,
    Unicode,
}

#[derive(Clone, Debug)]
pub(super) struct Part {
    pub participant: Participant,
    pub ordinal: usize,
    pub start: Position,
    pub end: Position,
    pub content: PartContent,
}

#[derive(Clone, Debug)]
pub(super) enum PartContent {
    Marker(Arc<Marker>),
    /// The `IMarkdownString`s of one hover.
    Markdown(Vec<Value>),
}

/// `ContentHoverResult` as shown, with the widget's `_visibleData` for `isMouseGettingCloser`.
pub(super) struct Shown {
    pub anchor: Anchor,
    pub parts: Vec<Part>,
    complete: bool,
    keyboard: bool,
    initial_mouse: Option<Point<f32>>,
    closest_distance: Option<f32>,
    /// The editor's scroll when shown: `_onEditorScrollChanged` hides on a change.
    scroll: (f32, f32),
}

/// `editor.hover` of the model's language.
pub(super) struct HoverSettings {
    enabled: String,
    delay: u64,
    sticky: bool,
    hiding_delay: u64,
    pub above: bool,
}

/// markdownHoverParticipant.ts `createLoadingMessage`.
const LOADING_ORDINAL: usize = 2000;

impl Workbench {
    pub(super) fn hover_settings(&self, editor: &OpenEditor) -> HoverSettings {
        let settings = self.settings.lock().unwrap();
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let value = |key: &str| settings.get_value(Some(&format!("editor.hover.{key}")), &overrides).unwrap_or(Value::Null);
        HoverSettings {
            enabled: value("enabled").as_str().unwrap_or("on").to_owned(),
            delay: value("delay").as_u64().unwrap_or(300),
            sticky: value("sticky").as_bool().unwrap_or(true),
            hiding_delay: value("hidingDelay").as_u64().unwrap_or(300),
            above: value("above").as_bool().unwrap_or(true),
        }
    }

    /// `editorHoverVisible`.
    pub(super) fn content_hover_visible(&self) -> bool {
        self.content_hover.shown.is_some()
    }

    /// `_onEditorMouseMove`: a move over the text area or the widget.
    pub(super) fn content_hover_mouse_moved(&mut self, target: MouseTarget, position: Point<Pixels>, modifiers: Modifiers, pressed: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let settings = self.hover_settings(editor);
        let on_widget = matches!(target, MouseTarget::Widget);
        // A drag that selects text hides the hover and computes none.
        if pressed && !on_widget {
            self.content_hover.reschedule = None;
            self.hide_content_hover(cx);
            return;
        }
        let mouse = MouseMove { target, position: position.map(f32::from), modifiers };
        self.content_hover.last_move = Some(mouse);
        let visible_from_keyboard = self.content_hover.shown.as_ref().is_some_and(|shown| shown.keyboard);
        if settings.sticky && (on_widget || visible_from_keyboard) {
            self.content_hover.reschedule = None;
            return;
        }
        // `_shouldRescheduleHoverComputation`: a grace period before a visible sticky hover reacts.
        if self.content_hover.shown.is_some() && settings.sticky && settings.hiding_delay > 0 {
            if self.content_hover.reschedule.is_none() {
                let delay = Duration::from_millis(settings.hiding_delay);
                self.content_hover.reschedule = Some(cx.spawn(async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.content_hover.reschedule = None;
                        if let Some(mouse) = workbench.content_hover.last_move {
                            workbench.react_to_mouse_move(mouse, cx);
                        }
                    });
                }));
            }
            return;
        }
        self.react_to_mouse_move(mouse, cx);
    }

    /// `_reactToEditorMouseMove`: `shouldShowHover` for `editor.hover.enabled`, then `showsOrWillShow`.
    fn react_to_mouse_move(&mut self, mouse: MouseMove, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let settings = self.hover_settings(editor);
        let show = match settings.enabled.as_str() {
            "on" => true,
            "off" => false,
            // isTriggerModifierPressed: the modifier `editor.multiCursorModifier` leaves free.
            _ => match self.setting("editor.multiCursorModifier") == "alt" {
                true => mouse.modifiers.platform,
                false => mouse.modifiers.alt,
            },
        };
        if show {
            let anchor = match mouse.target {
                MouseTarget::Text { line, visible } => anchor_at(editor, line, visible),
                MouseTarget::Widget | MouseTarget::Other => None,
            };
            if self.start_showing_or_update_hover(anchor, Mode::Delayed, false, Some(mouse.position), cx) {
                return;
            }
        }
        self.hide_content_hover(cx);
    }

    /// `_onKeyDown` with `editor.hover.enabled` at `onKeyboardModifier`: pressing the trigger modifier shows the
    /// hover for the last mouse move.
    pub(super) fn content_hover_modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if self.hover_settings(editor).enabled != "onKeyboardModifier" || self.content_hover.shown.is_some() {
            return;
        }
        if let Some(mouse) = self.content_hover.last_move {
            self.react_to_mouse_move(MouseMove { modifiers, ..mouse }, cx);
        }
    }

    /// `_onEditorMouseLeave`: leaving the text area for anywhere but the widget hides it.
    pub(super) fn content_hover_mouse_left(&mut self, mouse: Point<Pixels>, cx: &mut Context<Self>) {
        self.content_hover.reschedule = None;
        self.content_hover.last_move = None;
        let mouse = mouse.map(f32::from);
        if self.content_hover.widget_bounds.get().is_some_and(|bounds| self.content_hover.shown.is_some() && bounds.contains(&mouse)) {
            return;
        }
        self.hide_content_hover(cx);
    }

    /// `_onKeyDown`: a key hides the hover unless it starts a chord or shows the hover.
    pub(super) fn content_hover_key_down(&mut self, resolution: &crate::keybindings::Resolution, cx: &mut Context<Self>) {
        let potential_shortcut = match resolution {
            crate::keybindings::Resolution::MoreChordsNeeded => true,
            crate::keybindings::Resolution::KbFound { command, .. } => command == "editor.action.showHover" && self.content_hover.shown.is_some(),
            _ => false,
        };
        if !potential_shortcut {
            self.hide_content_hover(cx);
        }
    }

    /// `hideContentHover`: the operation stops and the widget goes.
    pub(super) fn hide_content_hover(&mut self, cx: &mut Context<Self>) {
        self.content_hover.operation = None;
        self.set_content_hover_result(None, cx);
    }

    /// The controller's editor listeners, run on every render: `onDidChangeModel` and `_onEditorScrollChanged` hide
    /// the hover.
    pub(super) fn follow_content_hover_editor(&mut self) {
        let Some(shown) = &self.content_hover.shown else { return };
        let current = self.editors.active().map(|editor| (editor.model_id, (editor.scroll.top, editor.scroll.left)));
        if current != Some((shown.anchor.model_id, shown.scroll)) {
            self.content_hover.operation = None;
            self.content_hover.reschedule = None;
            self.content_hover.shown = None;
            self.content_hover.widget_bounds.set(None);
        }
    }

    fn editor_scroll(&self) -> (f32, f32) {
        self.editors.active().map_or((0., 0.), |editor| (editor.scroll.top, editor.scroll.left))
    }

    pub(super) fn register_content_hover_commands(&mut self) {
        // hoverActions.ts ShowOrFocusHoverAction, with `focus` FocusIfVisible.
        // shortcut: the hover takes no keyboard focus yet, so a second run leaves it as it is; port its focus and
        // scroll commands with the hover's keyboard navigation.
        self.commands.register("editor.action.showHover", |workbench, _, _, cx| {
            if workbench.content_hover.shown.is_none()
                && let Some(editor) = workbench.editors.active()
            {
                let head = editor.selection().head;
                let anchor = Anchor { model_id: editor.model_id, line: head.line, start: head.column, end: head.column };
                workbench.start_showing_or_update_hover(Some(anchor), Mode::Immediate, true, None, cx);
            }
            crate::commands::Outcome::done()
        });
        self.commands.register("editor.action.hideHover", |workbench, _, _, cx| {
            workbench.hide_content_hover(cx);
            crate::commands::Outcome::done()
        });
    }

    /// `_startShowingOrUpdateHover`: whether the hover shows or will show for `anchor`.
    fn start_showing_or_update_hover(&mut self, anchor: Option<Anchor>, mode: Mode, keyboard: bool, mouse: Option<Point<f32>>, cx: &mut Context<Self>) -> bool {
        if self.content_hover.shown.is_none() {
            let Some(anchor) = anchor else { return false };
            self.start_hover_operation(anchor, mode, keyboard, false, mouse, cx);
            return true;
        }
        let sticky = self.editors.active().is_some_and(|editor| self.hover_settings(editor).sticky);
        if sticky && mouse.is_some_and(|mouse| self.mouse_getting_closer(mouse)) {
            if let Some(anchor) = anchor {
                self.start_hover_operation(anchor, mode, keyboard, true, mouse, cx);
            }
            return true;
        }
        let Some(anchor) = anchor else {
            self.set_content_hover_result(None, cx);
            return false;
        };
        let shown = self.content_hover.shown.as_ref().expect("checked above");
        if shown.anchor == anchor {
            return true;
        }
        // HoverRangeAnchor.canAdoptVisibleHover: the parts still covering the anchor on the same line stay.
        if shown.anchor.model_id != anchor.model_id || shown.anchor.line != anchor.line {
            self.set_content_hover_result(None, cx);
            self.start_hover_operation(anchor, mode, keyboard, false, mouse, cx);
            return true;
        }
        let parts = shown.parts.iter().filter(|part| part.start.column <= anchor.start && part.end.column >= anchor.end).cloned().collect();
        let complete = shown.complete;
        let scroll = self.editor_scroll();
        self.set_content_hover_result(Some(Shown { anchor, parts, complete, keyboard, initial_mouse: mouse, closest_distance: None, scroll }), cx);
        self.start_hover_operation(anchor, mode, keyboard, false, mouse, cx);
        true
    }

    /// contentHoverWidget.ts `isMouseGettingCloser`: the first move sets where the mouse started, later ones may
    /// stray 4px from the closest the mouse got to the widget.
    fn mouse_getting_closer(&mut self, mouse: Point<f32>) -> bool {
        let bounds = self.content_hover.widget_bounds.get();
        let Some(shown) = self.content_hover.shown.as_mut() else { return false };
        let Some(initial) = shown.initial_mouse else {
            shown.initial_mouse = Some(mouse);
            return false;
        };
        let Some(bounds) = bounds else { return false };
        let closest = *shown.closest_distance.get_or_insert_with(|| distance_to_rectangle(initial, bounds));
        let distance = distance_to_rectangle(mouse, bounds);
        if distance > closest + 4. {
            return false;
        }
        shown.closest_distance = Some(closest.min(distance));
        true
    }

    /// `_startHoverOperationIfNecessary` and `HoverOperation.start`.
    fn start_hover_operation(&mut self, anchor: Anchor, mode: Mode, keyboard: bool, insist: bool, mouse: Option<Point<f32>>, cx: &mut Context<Self>) {
        if self.content_hover.operation.as_ref().is_some_and(|operation| operation.anchor == anchor && operation.keyboard == keyboard) {
            return;
        }
        let Some(editor) = self.editors.active() else { return };
        let delay = self.hover_settings(editor).delay;
        self.content_hover.serial += 1;
        let serial = self.content_hover.serial;
        let state = if mode == Mode::Delayed { State::FirstWait } else { State::WaitingForAsync };
        self.content_hover.operation = Some(Operation { anchor, keyboard, insist, initial_mouse: mouse, state, pending_providers: 0, parts: Vec::new(), serial, _tasks: Vec::new() });
        if mode == Mode::Immediate {
            self.trigger_async_hover(serial, cx);
            self.trigger_sync_hover(serial, cx);
            return;
        }
        // hoverOperation.ts: providers start after half the delay, the sync parts come at the delay, and
        // "Loading..." shows at three times the delay.
        let first_wait = Duration::from_millis(delay / 2);
        let second_wait = Duration::from_millis(delay - delay / 2);
        let loading_wait = Duration::from_millis(3 * delay).saturating_sub(first_wait + second_wait);
        let task = cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(first_wait).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.trigger_async_hover(serial, cx));
            cx.background_executor().timer(second_wait).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.trigger_sync_hover(serial, cx));
            cx.background_executor().timer(loading_wait).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.trigger_hover_loading(serial, cx));
        });
        if let Some(operation) = self.content_hover.operation.as_mut() {
            operation._tasks.push(task);
        }
    }

    fn operation(&mut self, serial: usize) -> Option<&mut Operation> {
        self.content_hover.operation.as_mut().filter(|operation| operation.serial == serial)
    }

    /// `_triggerAsyncComputation`: markdownHoverParticipant.ts `computeAsync`, each provider's `$provideHover` in
    /// `ordered` order; a dropped operation cancels the calls still out.
    fn trigger_async_hover(&mut self, serial: usize, cx: &mut Context<Self>) {
        let Some(anchor) = self.operation(serial).map(|operation| operation.anchor) else { return };
        let mut calls = Vec::new();
        if let Some(editor) = self.editors.active().filter(|editor| editor.model_id == anchor.model_id) {
            let uri = editor.document.uri();
            let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
            let column = utf16_column(&editor.document.lines[anchor.line], anchor.start);
            let position = json!({ "lineNumber": anchor.line + 1, "column": column + 1 });
            for (ordinal, registration) in self.language_features.ordered("$registerHoverProvider", &candidate).into_iter().enumerate() {
                let args = vec![json!(registration.handle), uri.clone(), position.clone(), json!({})];
                calls.push((ordinal, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideHover", args)));
            }
        }
        let mut tasks = Vec::new();
        for (ordinal, call) in calls {
            tasks.push(cx.spawn(async move |workbench, cx| {
                let answer = call.await;
                let _ = workbench.update(cx, |workbench, cx| workbench.hover_provider_answered(serial, ordinal, answer, cx));
            }));
        }
        let Some(operation) = self.operation(serial) else { return };
        operation.pending_providers = tasks.len();
        operation._tasks.extend(tasks);
        if operation.state == State::FirstWait {
            operation.state = State::SecondWait;
        }
    }

    /// One provider's hover: getHover.ts `isValid` keeps only one with a range and contents.
    fn hover_provider_answered(&mut self, serial: usize, ordinal: usize, answer: Result<Result<Value, Value>, oneshot::Canceled>, cx: &mut Context<Self>) {
        let Some(anchor) = self.operation(serial).map(|operation| operation.anchor) else { return };
        let part = match (answer, self.editors.active()) {
            (Ok(Ok(hover)), Some(editor)) if editor.model_id == anchor.model_id && hover["range"].is_object() && !crate::markdown::is_empty(&hover["contents"]) => {
                let (start, end) = super::host_requests::range(editor, &hover["range"]);
                let contents = hover["contents"].as_array().cloned().unwrap_or_default();
                Some(Part { participant: Participant::Markdown, ordinal, start, end, content: PartContent::Markdown(contents) })
            }
            _ => None,
        };
        let operation = self.operation(serial).expect("checked above");
        operation.pending_providers -= 1;
        if let Some(part) = part {
            operation.parts.push(part);
            self.fire_hover_result(serial, cx);
        }
        let operation = self.operation(serial).expect("checked above");
        if operation.pending_providers == 0 && matches!(operation.state, State::WaitingForAsync | State::WaitingForAsyncShowingLoading) {
            operation.state = State::Idle;
            self.fire_hover_result(serial, cx);
        }
    }

    /// `_triggerSyncComputation`: the marker and unicode parts of the anchor's line decorations.
    fn trigger_sync_hover(&mut self, serial: usize, cx: &mut Context<Self>) {
        let Some(anchor) = self.operation(serial).map(|operation| operation.anchor) else { return };
        let parts = match self.editors.active().filter(|editor| editor.model_id == anchor.model_id) {
            Some(editor) => {
                let mut parts = marker_parts(editor, anchor);
                parts.extend(self.unicode_parts(editor, anchor));
                parts
            }
            None => Vec::new(),
        };
        let operation = self.operation(serial).expect("checked above");
        operation.parts.extend(parts);
        operation.state = if operation.pending_providers == 0 { State::Idle } else { State::WaitingForAsync };
        self.fire_hover_result(serial, cx);
    }

    fn trigger_hover_loading(&mut self, serial: usize, cx: &mut Context<Self>) {
        let Some(operation) = self.operation(serial) else { return };
        if operation.state == State::WaitingForAsync {
            operation.state = State::WaitingForAsyncShowingLoading;
            self.fire_hover_result(serial, cx);
        }
    }

    /// `_fireResult` and the wrapper's `_withResult`: a visible complete hover stays until the new one is complete.
    fn fire_hover_result(&mut self, serial: usize, cx: &mut Context<Self>) {
        let Some(operation) = self.operation(serial) else { return };
        if matches!(operation.state, State::FirstWait | State::SecondWait) {
            return;
        }
        let anchor = operation.anchor;
        let mut parts = operation.parts.clone();
        if operation.state == State::WaitingForAsyncShowingLoading {
            let (start, end) = (Position::new(anchor.line, anchor.start), Position::new(anchor.line, anchor.end));
            parts.push(Part { participant: Participant::Markdown, ordinal: LOADING_ORDINAL, start, end, content: PartContent::Markdown(vec![json!({ "value": "Loading..." })]) });
        }
        let complete = operation.state == State::Idle;
        let (insist, keyboard, initial_mouse) = (operation.insist, operation.keyboard, operation.initial_mouse);
        let scroll = self.editor_scroll();
        let result = || Shown { anchor, parts: parts.clone(), complete, keyboard, initial_mouse, closest_distance: None, scroll };
        if !self.content_hover.shown.as_ref().is_some_and(|shown| shown.complete) {
            self.set_content_hover_result(Some(result()), cx);
            if !complete {
                return;
            }
        }
        if !complete || (parts.is_empty() && insist) {
            return;
        }
        self.set_content_hover_result(Some(result()), cx);
    }

    /// `_setCurrentResult`: an empty result hides the widget.
    fn set_content_hover_result(&mut self, result: Option<Shown>, cx: &mut Context<Self>) {
        let result = result.filter(|result| !result.parts.is_empty());
        if self.content_hover.shown.is_none() && result.is_none() {
            return;
        }
        if self.content_hover.shown.is_none() {
            self.content_hover.shows += 1;
        }
        if result.is_none() {
            self.content_hover.widget_bounds.set(None);
        }
        self.content_hover.shown = result;
        self.code_actions_hover_changed(cx);
        cx.notify();
    }

    /// unicodeHighlighter.ts UnicodeHighlighterHoverParticipant `computeSync`: one part per reason.
    fn unicode_parts(&self, editor: &OpenEditor, anchor: Anchor) -> Vec<Part> {
        let Some((options, highlights)) = self.unicode_highlights(editor).filter(|(_, highlights)| !highlights.has_more) else { return Vec::new() };
        let highlighter = CodePointHighlighter::new(&options);
        let text = &editor.document.lines[anchor.line];
        let mut reasons = Vec::new();
        let mut parts = Vec::new();
        for (start, end) in highlights.ranges.iter().filter(|(start, _)| start.line == anchor.line) {
            if start.column > anchor.start || anchor.end > end.column {
                continue;
            }
            let Some(c) = text.chars().nth(start.column) else { continue };
            let Some(reason) = highlighter.reason(c, None) else { continue };
            let reason_text = super::unicode_highlight::unicode_hover_reason(c, reason, &highlighter);
            if reasons.contains(&reason_text) {
                continue;
            }
            reasons.push(reason_text.clone());
            let offset = text.char_indices().nth(start.column).map_or(text.len(), |(offset, _)| offset);
            let token = editor.document.tokens.token_type_at(&editor.document.lines, anchor.line, offset);
            let args = json!({
                "codePoint": c as u32,
                "reason": unicode_reason_value(reason, &highlighter, &options.allowed_locales, c),
                "inComment": token == textmate::StandardTokenType::Comment,
                "inString": token == textmate::StandardTokenType::String,
            });
            let uri = format!("command:editor.action.unicodeHighlight.showExcludeOptions?{}", encode_uri_component(&args.to_string()));
            let value = format!("{reason_text}&nbsp;[Adjust settings]({uri} \"Configure Unicode Highlight Options\")");
            let content = PartContent::Markdown(vec![json!({ "value": value, "isTrusted": true })]);
            parts.push(Part { participant: Participant::Unicode, ordinal: 300 + parts.len(), start: *start, end: *end, content });
        }
        parts
    }
}

/// markerHoverParticipant.ts `computeSync` over contentHoverComputer.ts `_getLineDecorations`: marker
/// decorations are `showIfCollapsed`, so they take an anchor one column beyond either end.
fn marker_parts(editor: &OpenEditor, anchor: Anchor) -> Vec<Part> {
    let line = anchor.line;
    let max_column = editor.document.lines[line].chars().count();
    let mut parts = Vec::new();
    for decoration in editor.markers.iter().filter(|decoration| decoration.start.line <= line && decoration.end.line >= line) {
        let start = if decoration.start.line == line { decoration.start.column } else { 0 };
        let end = if decoration.end.line == line { decoration.end.column } else { max_column };
        if start > anchor.start + 1 || anchor.end as isize - 1 > end as isize {
            continue;
        }
        // MarkerSeverity.compare sorts the most severe first.
        let ordinal = 16 - decoration.marker.severity as usize;
        parts.push(Part { participant: Participant::Marker, ordinal, start: Position::new(line, start), end: Position::new(line, end), content: PartContent::Marker(decoration.marker.clone()) });
    }
    parts
}

/// contentHoverWidgetWrapper.ts `_findHoverAnchorCandidates` for a mouse target: the char under the mouse, or the
/// end of the line when the mouse is less than half a char past it (mouseTarget.ts `createMouseTargetFromHitTestPosition`).
fn anchor_at(editor: &OpenEditor, line: usize, visible: f32) -> Option<Anchor> {
    let text = editor.document.lines.get(line)?;
    let tab_size = editor.document.indentation.tab_size;
    let count = text.chars().count();
    let width = visible_column(text, count, tab_size) as f32;
    let empty = |column| Anchor { model_id: editor.model_id, line, start: column, end: column };
    if visible >= width {
        return (visible - width < 0.5).then(|| empty(count));
    }
    let mut column_left = 0;
    for (index, c) in text.chars().enumerate() {
        column_left += if c == '\t' { tab_size - column_left % tab_size } else { 1 };
        if visible < column_left as f32 {
            return Some(Anchor { model_id: editor.model_id, line, start: index, end: index + 1 });
        }
    }
    Some(empty(count))
}

/// `computeDistanceFromPointToRectangle`.
fn distance_to_rectangle(point: Point<f32>, bounds: Bounds<f32>) -> f32 {
    let x = point.x.clamp(bounds.origin.x, bounds.origin.x + bounds.size.width);
    let y = point.y.clamp(bounds.origin.y, bounds.origin.y + bounds.size.height);
    ((point.x - x).powi(2) + (point.y - y).powi(2)).sqrt()
}

/// `formatCodePointMarkdown`: the `U+XXXX` code and, for a visible char, the char as inline code.
pub(super) fn format_code_point_markdown(c: char) -> String {
    let code = format!("`{}`", code_point_to_hex(c));
    match crate::editor::unicode_highlighter::is_invisible_character(c) {
        true => code,
        false if c == '`' => format!("{code} \"`` ` ``\""),
        false => format!("{code} \"`{c}`\""),
    }
}

/// `codePointToHex`: lowercase, as `toString(16)` writes it.
pub(super) fn code_point_to_hex(c: char) -> String {
    format!("U+{:04x}", c as u32)
}

/// `UnicodeHighlighterReason` as `ShowExcludeOptionsArgs` carries it.
fn unicode_reason_value(reason: HighlightReason, highlighter: &CodePointHighlighter, allowed_locales: &[String], c: char) -> Value {
    match reason {
        HighlightReason::Ambiguous => {
            let confusable = highlighter.ambiguous.get(&(c as u32)).and_then(|code| char::from_u32(*code)).unwrap_or(c);
            json!({ "kind": 0, "confusableWith": confusable.to_string(), "notAmbiguousInLocales": not_ambiguous_in_locales(c, allowed_locales) })
        }
        HighlightReason::Invisible => json!({ "kind": 1 }),
        HighlightReason::NonBasicAscii => json!({ "kind": 2 }),
    }
}

/// `encodeURIComponent`.
fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests;
