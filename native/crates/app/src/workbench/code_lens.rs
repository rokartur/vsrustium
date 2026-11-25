//! codelens.ts, codelensController.ts and codelensWidget.ts: the providers' lenses of the active model, each line's
//! lenses in a view zone above the line, resolved while they are in the viewport, their commands run on a click.

use std::time::Duration;

use futures::channel::oneshot;
use gpui::{AnyElement, Context, MouseButton, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_status::{StatusPick, label_item};
use super::quick_input_view::icon_at;
use crate::commands::Outcome;
use crate::editor::{OpenEditor, visible_column};
use crate::language_features::Candidate;
use crate::quick_input::{Entry, Kind, QuickInput};

const REGISTER: &str = "$registerCodeLensSupport";
/// The debouncers' minimum (`CodeLensProvide` and `CodeLensResolve`).
const DELAY: Duration = Duration::from_millis(250);
const UI_FONT: &str = ".SystemUIFont";
const EDITOR_FONT_SIZE: f32 = 12.;

#[derive(Default)]
pub(super) struct CodeLenses {
    model_id: Option<u32>,
    groups: Vec<LensGroup>,
    /// The providers' lists the shown lenses came from, released when replaced.
    caches: Vec<(u64, u64)>,
    provide: Option<Task<()>>,
    resolve: Option<Task<()>>,
}

/// One `CodeLensWidget`: the lenses of one line, the view zone above it, and what its content widget shows once
/// it has any command.
struct LensGroup {
    zone_id: u32,
    lenses: Vec<Lens>,
    shown: Option<Vec<Option<Value>>>,
}

#[derive(Clone)]
struct Lens {
    handle: u64,
    symbol: Value,
}

impl Workbench {
    pub(super) fn register_code_lens(&mut self) {
        self.commands.register("codelens.showLensesInCurrentLine", |workbench, _, window, cx| {
            workbench.show_lenses_in_current_line(window, cx);
            Outcome::done()
        });
        self.commands.register("_executeCodeLensProvider", |workbench, args, _, cx| workbench.execute_code_lens_provider(&args, cx));
    }

    /// `editorHasCodeLensProvider`.
    pub(super) fn code_lens_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        context.insert("editorHasCodeLensProvider".into(), json!(!self.language_features.ordered(REGISTER, &candidate).is_empty()));
    }

    /// `_onModelChange`: another model drops the lenses and asks its providers.
    // shortcut: no `CodeLensCache`, so a model shown again waits for its providers before its lenses show; keep the
    // last lenses per model (and in the workspace storage) to show them at once.
    pub(super) fn code_lens_model_changed(&mut self, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if model_id == self.code_lenses.model_id {
            return;
        }
        self.dispose_code_lenses();
        self.code_lenses.model_id = model_id;
        self.schedule_code_lenses(cx);
    }

    /// `onDidChangeModelContent`: the lenses ask again and pending resolves stop; a group whose line now holds
    /// another group's lenses goes.
    pub(super) fn code_lens_content_changed(&mut self, cx: &mut Context<Self>) {
        if self.code_lenses.groups.is_empty() && self.code_lenses.provide.is_none() {
            return;
        }
        if let Some(editor) = lens_editor(&mut self.editors, self.code_lenses.model_id) {
            let mut last_line = None;
            let mut removed = Vec::new();
            for (index, group) in self.code_lenses.groups.iter().enumerate() {
                let line = editor.view_zone(group.zone_id).map(|zone| zone.anchor.line);
                if line.is_none() || line == last_line {
                    removed.push(index);
                } else {
                    last_line = line;
                }
            }
            for index in removed.into_iter().rev() {
                let group = self.code_lenses.groups.remove(index);
                editor.remove_view_zone(group.zone_id);
            }
        }
        self.code_lenses.resolve = None;
        self.schedule_code_lenses(cx);
    }

    /// `onDidScrollChange`: the lenses that came into view resolve soon.
    pub(super) fn code_lens_scrolled(&mut self, cx: &mut Context<Self>) {
        if !self.code_lenses.groups.is_empty() {
            self.resolve_code_lenses_soon(cx);
        }
    }

    /// The registry changed (`onDidChange` of `codeLensProvider`): the model asks again, or drops its lenses when
    /// no provider is left.
    pub(super) fn code_lens_registrations_changed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        match self.language_features.ordered(REGISTER, &candidate).is_empty() {
            true => self.dispose_code_lenses(),
            false => self.schedule_code_lenses(cx),
        }
    }

    /// `$emitCodeLensEvent`: a provider of the model changed its lenses.
    pub(crate) fn code_lens_provider_changed(&mut self, event_handle: u64, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        if self.language_features.ordered(REGISTER, &candidate).iter().any(|registration| registration.args.get(2) == Some(&json!(event_handle))) {
            self.schedule_code_lenses(cx);
        }
    }

    /// `_disposeAllLenses` and the lists they came from.
    fn dispose_code_lenses(&mut self) {
        let groups = std::mem::take(&mut self.code_lenses.groups);
        if let Some(editor) = lens_editor(&mut self.editors, self.code_lenses.model_id) {
            for group in &groups {
                editor.remove_view_zone(group.zone_id);
            }
        }
        let caches = std::mem::take(&mut self.code_lenses.caches);
        self.release_code_lens_lists(caches);
        self.code_lenses.provide = None;
        self.code_lenses.resolve = None;
    }

    fn release_code_lens_lists(&self, caches: Vec<(u64, u64)>) {
        for (handle, cache_id) in caches {
            self.host.send("ExtHostLanguageFeatures", "$releaseCodeLenses", vec![json!(handle), json!(cache_id)]);
        }
    }

    /// The `RunOnceScheduler` that asks the providers 250ms after the last request for it.
    fn schedule_code_lenses(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.code_lenses.model_id) else { return };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let handles: Vec<u64> = self.language_features.ordered(REGISTER, &candidate).iter().map(|registration| registration.handle).collect();
        if handles.is_empty() || self.suggest_setting(editor, "editor.codeLens") == json!(false) {
            return;
        }
        self.code_lenses.provide = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(DELAY).await;
            let Ok(calls) = workbench.update(cx, |workbench, _| {
                handles.iter().map(|&handle| (handle, workbench.host.call_cancellable("ExtHostLanguageFeatures", "$provideCodeLenses", vec![json!(handle), uri.clone()]))).collect::<Vec<_>>()
            }) else {
                return;
            };
            let (lenses, caches) = collect_code_lenses(calls).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.code_lenses.provide = None;
                let old = std::mem::replace(&mut workbench.code_lenses.caches, caches);
                workbench.render_code_lenses(lenses);
                workbench.release_code_lens_lists(old);
                workbench.resolve_code_lenses_soon(cx);
                cx.notify();
            });
        }));
    }

    /// `_renderCodeLensSymbols`: the lenses grouped by line; a group on a line that had one keeps its zone and what
    /// it shows, the others get a zone; the lines in view stay where they were (`StableEditorScrollState`).
    fn render_code_lenses(&mut self, lenses: Vec<Lens>) {
        let viewport = self.editors.viewport;
        let height = self.code_lens_height();
        let mut old = std::mem::take(&mut self.code_lenses.groups);
        let Some(editor) = lens_editor(&mut self.editors, self.code_lenses.model_id) else { return };
        let line_count = editor.document.lines.len();
        let mut groups: Vec<(usize, Vec<Lens>)> = Vec::new();
        for lens in lenses {
            let line = lens.symbol["range"]["startLineNumber"].as_u64().unwrap_or(0) as usize;
            if line < 1 || line > line_count {
                continue;
            }
            match groups.last_mut() {
                Some((last, group)) if *last == line - 1 => group.push(lens),
                _ => groups.push((line - 1, vec![lens])),
            }
        }
        if groups.is_empty() && old.is_empty() {
            return;
        }
        // `StableEditorScrollState` leaves an editor at the top of the file alone.
        let scroll_top = editor.scroll.top;
        let first_view_line = editor.view_line_at(scroll_top, viewport);
        let offset = scroll_top - editor.line_top(first_view_line, viewport);
        let mut new_groups = Vec::new();
        for (line, lenses) in groups {
            let existing = old.iter().position(|group| editor.view_zone(group.zone_id).is_some_and(|zone| zone.anchor.line == line));
            match existing {
                Some(index) => {
                    let group = old.remove(index);
                    new_groups.push(LensGroup { lenses, ..group });
                }
                None => {
                    let with_commands: Vec<Option<Value>> = lenses.iter().filter(|lens| !lens.symbol["command"].is_null()).map(|lens| Some(lens.symbol.clone())).collect();
                    let zone_id = editor.add_view_zone_before(line, height);
                    new_groups.push(LensGroup { zone_id, lenses, shown: (!with_commands.is_empty()).then_some(with_commands) });
                }
            }
        }
        for group in old {
            editor.remove_view_zone(group.zone_id);
        }
        if scroll_top != 0. {
            let top = editor.line_top(first_view_line, viewport) + offset;
            editor.set_scroll(top, editor.scroll.left, viewport);
        }
        self.code_lenses.groups = new_groups;
    }

    /// `_getLayoutInfo`: the lens font is `editor.codeLensFontSize`, or 90% of the editor's; the zone is that size
    /// times the editor's line height factor, at least 1.3.
    fn code_lens_font_size(&self) -> f32 {
        let editor = self.editors.active();
        let size = editor.map(|editor| self.suggest_setting(editor, "editor.codeLensFontSize")).and_then(|size| size.as_f64()).unwrap_or(0.) as f32;
        let font_size = EDITOR_FONT_SIZE * self.editor_zoom.multiplier();
        if size < 5. { (font_size * 0.9).floor() } else { size }
    }

    fn code_lens_height(&self) -> f32 {
        let font_size = EDITOR_FONT_SIZE * self.editor_zoom.multiplier();
        let factor = (self.editors.viewport.line_height / font_size).max(1.3);
        (self.code_lens_font_size() * factor).floor()
    }

    fn resolve_code_lenses_soon(&mut self, cx: &mut Context<Self>) {
        self.code_lenses.resolve = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.resolve_code_lenses_in_viewport(cx));
        }));
    }

    /// `_resolveCodeLensesInViewport`: the groups whose zone is in view resolve their lenses without a command;
    /// each group then shows its lenses, a failed resolve leaving a gap.
    fn resolve_code_lenses_in_viewport(&mut self, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.code_lenses.model_id) else { return };
        let mut requests = Vec::new();
        for (index, group) in self.code_lenses.groups.iter().enumerate() {
            let Some(top) = editor.view_zone_top(group.zone_id, viewport) else { continue };
            let height = editor.view_zone(group.zone_id).map_or(0., |zone| zone.height);
            if top + height <= editor.scroll.top || top >= editor.scroll.top + viewport.height {
                continue;
            }
            let calls: Vec<_> = group
                .lenses
                .iter()
                .map(|lens| match lens.symbol["command"].is_null() {
                    true => Err(self.host.call_cancellable("ExtHostLanguageFeatures", "$resolveCodeLens", vec![json!(lens.handle), lens.symbol.clone()])),
                    false => Ok(lens.symbol.clone()),
                })
                .collect();
            requests.push((index, group.zone_id, calls));
        }
        if requests.is_empty() {
            return;
        }
        self.code_lenses.resolve = Some(cx.spawn(async move |workbench, cx| {
            let mut resolved = Vec::new();
            for (index, zone_id, calls) in requests {
                let mut symbols = Vec::new();
                for call in calls {
                    symbols.push(match call {
                        Ok(symbol) => Some(symbol),
                        Err(call) => call.await.ok().and_then(Result::ok).filter(|symbol| !symbol.is_null()),
                    });
                }
                resolved.push((index, zone_id, symbols));
            }
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.code_lenses.resolve = None;
                for (index, zone_id, symbols) in resolved {
                    let Some(group) = workbench.code_lenses.groups.get_mut(index).filter(|group| group.zone_id == zone_id) else { continue };
                    for (lens, symbol) in group.lenses.iter_mut().zip(&symbols) {
                        if let Some(command) = symbol.as_ref().map(|symbol| &symbol["command"]).filter(|command| !command.is_null()) {
                            lens.symbol["command"] = command.clone();
                        }
                    }
                    group.shown = Some(symbols);
                }
                cx.notify();
            });
        }));
    }

    /// `CodeLensContentWidget`s over their zones, at the line's first non-whitespace column.
    // shortcut: the lenses show in the system font without `editor.codeLensFontFamily` and without the 100ms fade in.
    pub(super) fn code_lens_widgets(&self, editor: &OpenEditor, content_left: f32, char_width: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.code_lenses.model_id != Some(editor.model_id) {
            return Vec::new();
        }
        let viewport = self.editors.viewport;
        let font_size = self.code_lens_font_size();
        let height = self.code_lens_height();
        let foreground = self.color("editorCodeLens.foreground");
        let active = self.color("editorLink.activeForeground");
        let mut widgets = Vec::new();
        for group in &self.code_lenses.groups {
            let (Some(shown), Some(zone)) = (&group.shown, editor.view_zone(group.zone_id)) else { continue };
            let Some(top) = editor.view_zone_top(group.zone_id, viewport) else { continue };
            let text = &editor.document.lines[zone.anchor.line];
            let first = text.chars().position(|character| !character.is_whitespace()).unwrap_or(text.chars().count());
            let left = content_left + visible_column(text, first, editor.document.indentation.tab_size) as f32 * char_width - editor.scroll.left;
            let mut widget = div()
                .absolute()
                .left(px(left))
                .top(px(top - editor.scroll.top))
                .h(px(height))
                .pr(px(font_size * 0.5))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .overflow_hidden()
                .font_family(UI_FONT)
                .text_size(px(font_size))
                .line_height(px(height))
                .text_color(foreground);
            if !shown.iter().any(Option::is_some) {
                widgets.push(widget.child("no commands").into_any_element());
                continue;
            }
            for (index, symbol) in shown.iter().enumerate() {
                let Some(command) = symbol.as_ref().map(|symbol| &symbol["command"]).filter(|command| !command.is_null()) else { continue };
                let title = self.code_lens_title(command["title"].as_str().unwrap_or_default().trim(), font_size, foreground);
                match command["id"].as_str().filter(|id| !id.is_empty()) {
                    Some(id) => {
                        let (id, args) = (id.to_owned(), command["arguments"].as_array().cloned().unwrap_or_default());
                        let link = div()
                            .id(("code-lens", group.zone_id as usize * 1000 + index))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(move |style| style.text_color(active))
                            .child(title)
                            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                                cx.stop_propagation();
                                workbench.run_code_lens_command(&id, args.clone(), window, cx);
                            }));
                        widget = widget.child(link);
                    }
                    None => widget = widget.child(div().flex().items_center().child(title)),
                }
                if index + 1 < shown.len() {
                    widget = widget.child("\u{a0}|\u{a0}");
                }
            }
            widgets.push(widget.into_any_element());
        }
        widgets
    }

    /// `renderLabelWithIcons`: `$(name)` draws a codicon in the lens color.
    fn code_lens_title(&self, title: &str, font_size: f32, color: gpui::Rgba) -> gpui::Div {
        let chars: Vec<char> = title.chars().collect();
        let mut row = div().flex().items_center();
        let mut text = String::new();
        let mut index = 0;
        while index < chars.len() {
            if let Some((name, length)) = icon_at(&chars[index..]) {
                if !text.is_empty() {
                    row = row.child(std::mem::take(&mut text));
                }
                row = row.child(self.icon(&name, font_size, font_size, color));
                index += length;
                continue;
            }
            text.push(chars[index]);
            index += 1;
        }
        if !text.is_empty() {
            row = row.child(text);
        }
        row
    }

    /// A lens command runs; its failure is an error notification.
    pub(super) fn run_code_lens_command(&mut self, id: &str, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.execute_command(id, args, window, cx);
        let pending = match outcome {
            Outcome::Ready(Ok(_)) => return,
            Outcome::Ready(Err(error)) => return self.notify_error(error_message(&error)),
            Outcome::Pending(pending) => pending,
        };
        cx.spawn(async move |workbench, cx| {
            if let Ok(Err(error)) = pending.await {
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.notify_error(error_message(&error));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `ShowLensesInCurrentLine`: a pick of the resolved lens commands on the cursor's line.
    fn show_lenses_in_current_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.code_lenses.model_id) else { return };
        let line = editor.selection().head.line;
        let mut rows = Vec::new();
        for lens in self.code_lenses.groups.iter().flat_map(|group| &group.lenses) {
            let command = &lens.symbol["command"];
            if command.is_null() || lens.symbol["range"]["startLineNumber"].as_u64() != Some(line as u64 + 1) {
                continue;
            }
            let pick = StatusPick::ExtensionCommand(command["id"].as_str().unwrap_or_default().to_owned(), command["arguments"].as_array().cloned().unwrap_or_default());
            rows.push((Entry::Item(label_item(command["title"].as_str().unwrap_or_default())), pick));
        }
        if rows.is_empty() {
            return;
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a command".into());
        self.show_status_picker(input, rows, window, cx);
    }

    /// `_executeCodeLensProvider`: the lenses of a model; with `itemResolveCount`, the ones without a command up to
    /// that count resolved and the rest left out.
    fn execute_code_lens_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let uri = editor.document.uri();
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let calls: Vec<_> = self
            .language_features
            .ordered(REGISTER, &candidate)
            .iter()
            .map(|registration| (registration.handle, self.host.call("ExtHostLanguageFeatures", "$provideCodeLenses", vec![json!(registration.handle), uri.clone()])))
            .collect();
        let mut resolve_count = args.get(1).and_then(Value::as_u64);
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let (lenses, caches) = collect_code_lenses(calls).await;
            let mut result = Vec::new();
            for lens in lenses {
                let Some(count) = resolve_count.filter(|_| lens.symbol["command"].is_null()) else {
                    result.push(lens.symbol);
                    continue;
                };
                if count == 0 {
                    continue;
                }
                resolve_count = Some(count - 1);
                let Ok(call) = workbench.update(cx, |workbench, _| workbench.host.call("ExtHostLanguageFeatures", "$resolveCodeLens", vec![json!(lens.handle), lens.symbol.clone()])) else { return };
                let resolved = call.await.ok().and_then(Result::ok).filter(|symbol| !symbol.is_null());
                result.push(resolved.unwrap_or(lens.symbol));
            }
            let _ = sender.send(Ok(Value::Array(result)));
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = workbench.update(cx, |workbench, _| workbench.release_code_lens_lists(caches));
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

/// `getCodeLensModel` after the calls went out: the lenses by line, then provider order, then column.
async fn collect_code_lenses(calls: Vec<(u64, oneshot::Receiver<Result<Value, Value>>)>) -> (Vec<Lens>, Vec<(u64, u64)>) {
    let mut lenses = Vec::new();
    let mut caches = Vec::new();
    for (rank, (handle, call)) in calls.into_iter().enumerate() {
        let Ok(Ok(list)) = call.await else { continue };
        if let Some(cache_id) = list["cacheId"].as_u64() {
            caches.push((handle, cache_id));
        }
        for symbol in list["lenses"].as_array().into_iter().flatten() {
            lenses.push((rank, Lens { handle, symbol: symbol.clone() }));
        }
    }
    let start = |lens: &Lens, key: &str| lens.symbol["range"][key].as_u64().unwrap_or(0);
    lenses.sort_by_key(|(rank, lens)| (start(lens, "startLineNumber"), *rank, start(lens, "startColumn")));
    (lenses.into_iter().map(|(_, lens)| lens).collect(), caches)
}

/// The editor of the model the lenses are for.
fn lens_editor(editors: &mut crate::editor::EditorGroup, model_id: Option<u32>) -> Option<&mut OpenEditor> {
    editors.editors.iter_mut().find(|editor| Some(editor.model_id) == model_id)
}

fn error_message(error: &Value) -> String {
    error["message"].as_str().or(error.as_str()).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests;
