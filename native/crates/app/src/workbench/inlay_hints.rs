//! inlayHints.ts and inlayHintsController.ts: the providers' hints around the visible lines of the active editor,
//! drawn as injected text before or after their position; a label part with a command or location is a link under
//! the trigger modifier, a double click applies the hint's text edits; `_executeInlayHintProvider`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui::{AnyElement, Context, Modifiers, MouseButton, MouseDownEvent, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_view::EDITOR_FONT;
use crate::commands::Outcome;
use crate::editor::injected_text::{InjectedText, InjectionOwner};
use crate::editor::{OpenEditor, Position, char_column, utf16_column, word_at};
use crate::language_features::Candidate;

const REGISTER: &str = "$registerInlayHintsProvider";
/// The `InlayHint` feature debounce's minimum.
// shortcut: a fixed debounce, where `LanguageFeatureDebounceService` learns it from the providers' answer times.
const DELAY: Duration = Duration::from_millis(25);
/// `_updateHintsDecorators`: after an edit the hints before the cursor keep their lengths this long at least.
const FIXED_LENGTHS_DELAY: Duration = Duration::from_millis(800);
/// `InlayHintsController._MAX_DECORATORS`.
const MAX_DECORATORS: usize = 1500;
/// `InlayHintsCache`: an LRU of 50 models.
const CACHE_SIZE: usize = 50;
/// Menlo's advance is 1233/2048 em: the editor font's char width per px of font size, for the padding's px.
const EDITOR_CHAR_EM: f32 = 1233. / 2048.;
const EDITOR_FONT_SIZE: f32 = 12.;

#[derive(Default)]
pub(super) struct InlayHints {
    model_id: Option<u32>,
    items: Vec<HintItem>,
    /// The label parts each shown hint draws, by item index, as cut when its injected text was made.
    rendered: HashMap<usize, Vec<RenderedPart>>,
    /// The providers' lists the shown hints came from, released when replaced (`InlayHintList.dispose`).
    lists: Vec<(u64, u64)>,
    /// (model id, version) to the hints with their current positions, newest last.
    cache: Vec<((u32, u32), Vec<HintItem>)>,
    request: Option<Task<()>>,
    full_refresh: Option<Task<()>>,
    /// `_cursorInfo`: the cursor after an edit and until when the hints before it keep their lengths.
    cursor_info: Option<(Position, Instant)>,
    /// `_activeRenderMode` Invisible: `onUnlessPressed`/`offUnlessPressed` hide the hints.
    hidden: bool,
    /// `_activeInlayHintPart`: the label part under the pointer and whether the trigger modifier is down.
    active_part: Option<(usize, usize)>,
    modifiers: Modifiers,
    /// The event handles of the providers whose changes the hints follow.
    watched: Vec<u64>,
}

#[derive(Clone, Debug)]
struct HintItem {
    handle: u64,
    hint: Value,
    position: Position,
    /// `InlayHintAnchor.direction`: `after` when the word or token at the position starts before it.
    after: bool,
    resolved: bool,
}

/// One drawn label part: its text after truncation and the part's index in the label.
struct RenderedPart {
    text: String,
    index: usize,
}

impl Workbench {
    pub(super) fn register_inlay_hints(&mut self) {
        self.commands.register("_executeInlayHintProvider", |workbench, args, _, cx| workbench.execute_inlay_hint_provider(&args, cx));
    }

    /// `_update` on another model, language or provider set: the shown hints go, the cached ones of the model
    /// show at once and the providers are asked.
    pub(super) fn inlay_hints_model_changed(&mut self, cx: &mut Context<Self>) {
        let model_id = self.editors.active().map(|editor| editor.model_id);
        if model_id == self.inlay_hints.model_id {
            return;
        }
        self.cache_inlay_hints();
        self.inlay_hints_update(cx);
    }

    /// The registry or `editor.inlayHints` changed.
    pub(super) fn inlay_hints_registrations_changed(&mut self, cx: &mut Context<Self>) {
        self.cache_inlay_hints();
        self.inlay_hints_update(cx);
    }

    fn inlay_hints_update(&mut self, cx: &mut Context<Self>) {
        self.remove_inlay_hints();
        self.inlay_hints.request = None;
        self.inlay_hints.full_refresh = None;
        self.inlay_hints.watched.clear();
        let Some(editor) = self.editors.active() else {
            self.inlay_hints.model_id = None;
            return;
        };
        self.inlay_hints.model_id = Some(editor.model_id);
        let enabled = self.suggest_setting(editor, "editor.inlayHints.enabled");
        let enabled = enabled.as_str().unwrap_or("on");
        if enabled == "off" || self.inlay_hint_handles(editor).is_empty() {
            return;
        }
        self.inlay_hints.hidden = match enabled {
            "onUnlessPressed" => pressed(self.inlay_hints.modifiers),
            "offUnlessPressed" => !pressed(self.inlay_hints.modifiers),
            _ => false,
        };
        let key = (editor.model_id, editor.text_model().version_id);
        if let Some(index) = self.inlay_hints.cache.iter().position(|(cached, _)| *cached == key) {
            let (_, items) = self.inlay_hints.cache.remove(index);
            self.render_inlay_hints(items);
        }
        self.schedule_inlay_hints(Duration::ZERO, cx);
    }

    fn inlay_hint_handles(&self, editor: &OpenEditor) -> Vec<u64> {
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        // `registry.ordered(model).reverse()`.
        self.language_features.ordered(REGISTER, &candidate).iter().rev().map(|registration| registration.handle).collect()
    }

    /// `onDidScrollChange`.
    pub(super) fn inlay_hints_scrolled(&mut self, cx: &mut Context<Self>) {
        if self.inlay_hints.model_id.is_some() && self.inlay_hints.request.is_none() {
            self.schedule_inlay_hints(DELAY, cx);
        }
    }

    /// `onDidChangeModelContent`: ask again soon, and fully after the fixed lengths time.
    pub(super) fn inlay_hints_content_changed(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.inlay_hints.model_id) else { return };
        if self.inlay_hint_handles(editor).is_empty() {
            return;
        }
        self.inlay_hints.cursor_info = Some((editor.selection().head, Instant::now() + FIXED_LENGTHS_DELAY));
        self.inlay_hints.full_refresh = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(FIXED_LENGTHS_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| workbench.schedule_inlay_hints(Duration::ZERO, cx));
        }));
        self.schedule_inlay_hints(DELAY, cx);
    }

    /// `$emitInlayHintsEvent`: a provider whose hints showed changed them; ignored while a request is scheduled.
    pub(crate) fn inlay_hints_provider_changed(&mut self, event_handle: u64, cx: &mut Context<Self>) {
        if self.inlay_hints.watched.contains(&event_handle) && self.inlay_hints.request.is_none() {
            self.schedule_inlay_hints(DELAY, cx);
        }
    }

    /// `ModifierKeyEmitter`: Ctrl+Alt (without Shift or Cmd) switches the `...UnlessPressed` render modes; the
    /// trigger modifier makes the hovered label part a link.
    pub(super) fn inlay_hints_modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let was = self.inlay_hints.modifiers;
        self.inlay_hints.modifiers = modifiers;
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.inlay_hints.model_id) else { return };
        let hidden = match self.suggest_setting(editor, "editor.inlayHints.enabled").as_str() {
            Some("onUnlessPressed") => pressed(modifiers),
            Some("offUnlessPressed") => !pressed(modifiers),
            _ => false,
        };
        if hidden != self.inlay_hints.hidden {
            self.inlay_hints.hidden = hidden;
            let items = self.current_inlay_hints();
            self.render_inlay_hints(items);
            self.schedule_inlay_hints(Duration::ZERO, cx);
            cx.notify();
        } else if self.inlay_hints.active_part.is_some() && self.link_modifier(was) != self.link_modifier(modifiers) {
            cx.notify();
        }
    }

    fn schedule_inlay_hints(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active().filter(|editor| Some(editor.model_id) == self.inlay_hints.model_id) else { return };
        let handles = self.inlay_hint_handles(editor);
        let model_id = editor.model_id;
        let ranges = hint_ranges(editor, self.editors.viewport);
        self.inlay_hints.request = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(calls) = workbench.update(cx, |workbench, _| {
                let Some(editor) = workbench.editors.active().filter(|editor| editor.model_id == model_id) else { return Vec::new() };
                let uri = editor.document().uri();
                let mut calls = Vec::new();
                for &handle in &handles {
                    for range in &ranges {
                        calls.push((handle, workbench.host.call_cancellable("ExtHostLanguageFeatures", "$provideInlayHints", vec![json!(handle), uri.clone(), range.clone()])));
                    }
                }
                calls
            }) else {
                return;
            };
            let mut lists = Vec::new();
            for (handle, call) in calls {
                if let Ok(Ok(list)) = call.await
                    && !list.is_null()
                {
                    lists.push((handle, list));
                }
            }
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.inlay_hints.request = None;
                let Some(editor) = workbench.editors.active().filter(|editor| editor.model_id == model_id) else { return };
                let lines = editor.document().lines.clone();
                let mut items = Vec::new();
                let mut caches = Vec::new();
                for (handle, list) in lists {
                    if let Some(cache_id) = list["cacheId"].as_u64() {
                        caches.push((handle, cache_id));
                    }
                    for hint in list["hints"].as_array().into_iter().flatten() {
                        items.push(hint_item(&lines, handle, hint.clone()));
                    }
                }
                // `InlayHintsFragments` keeps the providers that answer; their events ask again.
                let watched: Vec<u64> = workbench.inlay_hints_event_handles();
                workbench.inlay_hints.watched = watched;
                items.sort_by_key(|item| item.position);
                let old = std::mem::replace(&mut workbench.inlay_hints.lists, caches);
                workbench.render_inlay_hints(items);
                workbench.release_inlay_hint_lists(old);
                workbench.cache_inlay_hints();
                cx.notify();
            });
        }));
    }

    fn inlay_hints_event_handles(&self) -> Vec<u64> {
        let Some(editor) = self.editors.active() else { return Vec::new() };
        let uri = editor.document().uri();
        let language = editor.document().language_id.clone();
        let candidate = Candidate { uri: &uri, language: &language };
        self.language_features.ordered(REGISTER, &candidate).iter().filter_map(|registration| registration.args.get(3).and_then(Value::as_u64)).collect()
    }

    fn release_inlay_hint_lists(&self, lists: Vec<(u64, u64)>) {
        for (handle, cache_id) in lists {
            self.host.send("ExtHostLanguageFeatures", "$releaseInlayHints", vec![json!(handle), json!(cache_id)]);
        }
    }

    /// `_cacheHintsForFastRestore` under the model's version.
    fn cache_inlay_hints(&mut self) {
        let Some(model_id) = self.inlay_hints.model_id else { return };
        let Some(editor) = hint_editor(&self.editors, model_id) else { return };
        let key = (model_id, editor.text_model().version_id);
        let items = self.current_inlay_hints();
        self.inlay_hints.cache.retain(|(cached, _)| *cached != key);
        self.inlay_hints.cache.push((key, items));
        if self.inlay_hints.cache.len() > CACHE_SIZE {
            self.inlay_hints.cache.remove(0);
        }
    }

    /// `_copyInlayHintsWithCurrentAnchor`: the shown hints at the positions their injected text moved to.
    fn current_inlay_hints(&self) -> Vec<HintItem> {
        let mut items = self.inlay_hints.items.clone();
        if let Some(editor) = self.inlay_hints.model_id.and_then(|model_id| hint_editor(&self.editors, model_id)) {
            for injection in editor.injected_text.iter().filter(|injection| injection.owner == InjectionOwner::InlayHint) {
                items[injection.id].position = injection.position;
            }
        }
        items
    }

    fn remove_inlay_hints(&mut self) {
        let lists = std::mem::take(&mut self.inlay_hints.lists);
        self.release_inlay_hint_lists(lists);
        self.inlay_hints.items.clear();
        self.inlay_hints.active_part = None;
        if let Some(model_id) = self.inlay_hints.model_id
            && let Some(editor) = hint_editor_mut(&mut self.editors, model_id)
        {
            editor.set_injected_text(InjectionOwner::InlayHint, Vec::new());
        }
    }

    /// `_updateHintsDecorators` over the whole model: one injected text per hint, its width the label's (cut at
    /// `maximumLength` per line, or at the length it had before the cursor right after an edit) plus padding.
    fn render_inlay_hints(&mut self, items: Vec<HintItem>) {
        let Some(model_id) = self.inlay_hints.model_id else { return };
        let Some(editor) = hint_editor(&self.editors, model_id) else { return };
        let layout = self.inlay_hint_layout(editor);
        let fixed = self.fixed_inlay_hint_lengths(&items);
        let mut injections = Vec::new();
        let mut rendered = HashMap::new();
        if !self.inlay_hints.hidden {
            let mut line_total = (usize::MAX, 0);
            for (id, item) in items.iter().enumerate() {
                if line_total.0 != item.position.line {
                    line_total = (item.position.line, 0);
                }
                if layout.maximum_length > 0 && line_total.1 > layout.maximum_length {
                    continue;
                }
                let parts = rendered_parts(&item.hint, &mut line_total.1, layout.maximum_length, fixed.get(&id).copied());
                let chars: usize = parts.iter().map(|part| part.text.chars().count()).sum();
                let columns = chars as f32 * layout.font_size / layout.editor_font_size + self.inlay_hint_padding_px(&item.hint, &layout) / (layout.editor_font_size * EDITOR_CHAR_EM);
                injections.push(InjectedText { position: item.position, columns, owner: InjectionOwner::InlayHint, id, at_range_start: !item.after });
                rendered.insert(id, parts);
                if injections.len() > MAX_DECORATORS {
                    break;
                }
            }
        }
        self.inlay_hints.items = items;
        self.inlay_hints.rendered = rendered;
        let editor = hint_editor_mut(&mut self.editors, model_id).expect("found above");
        let scroll = editor.scroll;
        editor.set_injected_text(InjectionOwner::InlayHint, injections);
        // `StableEditorScrollState`: the hints change no line's top, the scroll stays.
        editor.scroll = scroll;
    }

    /// The pixels of a hint outside its label text: `paddingLeft`/`paddingRight` whitespace of a third of the font
    /// size, and with `editor.inlayHints.padding` the label's quarter font size padding on each side.
    fn inlay_hint_padding_px(&self, hint: &Value, layout: &HintLayout) -> f32 {
        let whitespace = (layout.font_size / 3.).floor();
        let mut padding = 0.;
        if hint["paddingLeft"] == json!(true) {
            padding += whitespace;
        }
        if hint["paddingRight"] == json!(true) {
            padding += whitespace;
        }
        if layout.padding {
            padding += 2. * (layout.font_size / 4.).floor().max(1.);
        }
        padding
    }

    /// The fixed lengths of `_updateHintsDecorators`: within the time after an edit, the new hints on the cursor's
    /// line before it take the lengths the old ones there had, the rest given to the last.
    fn fixed_inlay_hint_lengths(&mut self, items: &[HintItem]) -> HashMap<usize, usize> {
        let mut fixed = HashMap::new();
        let Some((position, until)) = self.inlay_hints.cursor_info.take() else { return fixed };
        if Instant::now() > until {
            return fixed;
        }
        let old_items = self.current_inlay_hints();
        let Some(editor) = self.inlay_hints.model_id.and_then(|model_id| hint_editor(&self.editors, model_id)) else { return fixed };
        let mut lengths: Vec<usize> = Vec::new();
        for injection in editor.injected_text_on(position.line).iter().filter(|injection| injection.owner == InjectionOwner::InlayHint && injection.position.column <= position.column) {
            lengths.push(label_length(&old_items[injection.id].hint));
        }
        let mut targets = items.iter().enumerate().filter(|(_, item)| item.position.line == position.line && item.position.column <= position.column).map(|(id, _)| id);
        let mut last = None;
        let mut lengths = lengths.into_iter();
        loop {
            match (targets.next(), lengths.next()) {
                (None, None) => break,
                (Some(target), length) => {
                    fixed.insert(target, length.unwrap_or(0));
                    last = Some(target);
                }
                (None, Some(length)) => {
                    if let Some(last) = last {
                        let rest: usize = lengths.by_ref().sum();
                        *fixed.get_mut(&last).expect("inserted above") += length + rest;
                    }
                    break;
                }
            }
        }
        fixed
    }

    fn inlay_hint_layout(&self, editor: &OpenEditor) -> HintLayout {
        let editor_font_size = EDITOR_FONT_SIZE * self.editor_zoom.multiplier();
        // `_getLayoutInfo`: the editor's font size when the setting is under 5 or over the editor's.
        let size = self.suggest_setting(editor, "editor.inlayHints.fontSize").as_f64().unwrap_or(0.) as f32;
        let font_size = if size < 5. || size > editor_font_size { editor_font_size } else { size };
        let family = self.suggest_setting(editor, "editor.inlayHints.fontFamily");
        let family = family.as_str().filter(|family| !family.is_empty()).unwrap_or(EDITOR_FONT).to_owned();
        let padding = self.suggest_setting(editor, "editor.inlayHints.padding") == json!(true);
        let maximum_length = self.suggest_setting(editor, "editor.inlayHints.maximumLength").as_u64().unwrap_or(43) as usize;
        HintLayout { font_size, editor_font_size, family, padding, maximum_length }
    }

    /// The hint `id` drawn in the gap its injected text leaves: each label part a span in the hint font with the
    /// kind's colors, the active part underlined, a link under the trigger modifier.
    pub(super) fn inlay_hint_element(&self, editor: &OpenEditor, id: usize, left: f32, line_height: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let item = self.inlay_hints.items.get(id)?;
        let parts = self.inlay_hints.rendered.get(&id)?;
        let layout = self.inlay_hint_layout(editor);
        let hint = &item.hint;
        let (foreground, background) = match hint["kind"].as_u64() {
            Some(1) => ("editorInlayHint.typeForeground", "editorInlayHint.typeBackground"),
            Some(2) => ("editorInlayHint.parameterForeground", "editorInlayHint.parameterBackground"),
            _ => ("editorInlayHint.foreground", "editorInlayHint.background"),
        };
        let (foreground, background) = (self.color(foreground), self.theme.color(background));
        let active_link = self.color("editorLink.activeForeground");
        let whitespace = (layout.font_size / 3.).floor();
        let has_edits = hint["textEdits"].as_array().is_some_and(|edits| !edits.is_empty());
        let radius = (layout.font_size / 4.).floor();
        let side_padding = radius.max(1.);
        let link_modifier = self.link_modifier(self.inlay_hints.modifiers);
        let content_height = layout.font_size * 1.164;
        let height = content_height + if layout.padding { 2. } else { 0. };
        let mut row = div().absolute().left(px(left)).top(px(((line_height - height) / 2.).round())).h(px(height)).flex().items_center().whitespace_nowrap();
        if hint["paddingLeft"] == json!(true) {
            row = row.child(div().w(px(whitespace)).flex_none());
        }
        let count = parts.len();
        for (position, part) in parts.iter().enumerate() {
            let label_part = label_part(hint, part.index);
            let is_link = !label_part["command"].is_null() || !label_part["location"].is_null();
            let active = is_link && self.inlay_hints.active_part == Some((id, part.index));
            let mut span = div()
                .id(("inlay-hint", id * 1000 + part.index))
                .h_full()
                .flex()
                .items_center()
                .font_family(layout.family.clone())
                .text_size(px(layout.font_size))
                .text_color(if active && link_modifier { active_link } else { foreground })
                .when_some(background, |span, background| span.bg(background))
                .when(active, |span| span.underline())
                .when(has_edits, |span| span.cursor_default())
                .when(active && link_modifier, |span| span.cursor_pointer())
                .child(part.text.clone());
            if layout.padding {
                let (first, last) = (position == 0, position + 1 == count);
                span = span.when(first, |span| span.pl(px(side_padding)).rounded_l(px(radius))).when(last, |span| span.pr(px(side_padding)).rounded_r(px(radius)));
            }
            let part_index = part.index;
            span = span
                .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| workbench.inlay_hint_part_hovered(id, part_index, *hovered, cx)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.inlay_hint_part_clicked(id, part_index, event, window, cx)));
            row = row.child(span);
        }
        if hint["paddingRight"] == json!(true) {
            row = row.child(div().w(px(whitespace)).flex_none());
        }
        Some(row.into_any_element())
    }

    /// The link gesture's `onMouseMoveOrRelevantKeyDown`: the part under the pointer resolves its hint and becomes
    /// the active part when it has a command or location.
    fn inlay_hint_part_hovered(&mut self, id: usize, part: usize, hovered: bool, cx: &mut Context<Self>) {
        if !hovered {
            if self.inlay_hints.active_part == Some((id, part)) {
                self.inlay_hints.active_part = None;
                cx.notify();
            }
            return;
        }
        let Some(item) = self.inlay_hints.items.get(id) else { return };
        let label_part = label_part(&item.hint, part);
        let is_link = !label_part["command"].is_null() || !label_part["location"].is_null();
        self.inlay_hints.active_part = is_link.then_some((id, part));
        self.resolve_inlay_hint(id, cx);
        cx.notify();
    }

    /// `InlayHintItem.resolve`: the provider's resolved tooltip, label and text edits replace the hint's.
    fn resolve_inlay_hint(&mut self, id: usize, cx: &mut Context<Self>) -> Option<oneshot::Receiver<()>> {
        let item = self.inlay_hints.items.get_mut(id)?;
        let cache_id = item.hint["cacheId"].clone();
        if item.resolved || cache_id.is_null() {
            return None;
        }
        item.resolved = true;
        let supports_resolve = self.language_features.all(REGISTER).any(|registration| registration.handle == item.handle && registration.args.get(2) == Some(&json!(true)));
        if !supports_resolve {
            return None;
        }
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", "$resolveInlayHint", vec![json!(item.handle), cache_id.clone()]);
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let resolved = call.await.ok().and_then(Result::ok).filter(|hint| !hint.is_null());
            let _ = workbench.update(cx, |workbench, cx| {
                if let (Some(resolved), Some(item)) = (resolved, workbench.inlay_hints.items.get_mut(id).filter(|item| item.hint["cacheId"] == cache_id)) {
                    for key in ["tooltip", "label", "textEdits"] {
                        if !resolved[key].is_null() {
                            item.hint[key] = resolved[key].clone();
                        }
                    }
                }
                cx.notify();
            });
            let _ = sender.send(());
        })
        .detach();
        Some(receiver)
    }

    /// The link gesture's `onExecute` (trigger modifier and click): a location goes to the definition, a command
    /// runs; `_installDblClickGesture`: a double click applies the resolved hint's text edits.
    fn inlay_hint_part_clicked(&mut self, id: usize, part: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.inlay_hints.items.get(id) else { return };
        if event.click_count == 2 {
            cx.stop_propagation();
            let pending = self.resolve_inlay_hint(id, cx);
            cx.spawn(async move |workbench, cx| {
                if let Some(pending) = pending {
                    let _ = pending.await;
                }
                let _ = workbench.update(cx, |workbench, cx| workbench.apply_inlay_hint_edits(id, cx));
            })
            .detach();
            return;
        }
        if !self.link_modifier(event.modifiers) {
            return;
        }
        let label_part = label_part(&item.hint, part);
        if !label_part["location"].is_null() {
            cx.stop_propagation();
            let location = label_part["location"].clone();
            let Some(editor) = self.editors.active() else { return };
            let uri = editor.document().uri();
            let head = editor.selection().head;
            let position = json!({ "lineNumber": head.line + 1, "column": utf16_column(&editor.document().lines[head.line], head.column) + 1 });
            self.execute_command("editor.action.goToLocations", vec![uri, position, json!([location]), json!("goto"), json!("")], window, cx);
        } else if let Some(id) = label_part["command"]["id"].as_str() {
            cx.stop_propagation();
            let args = label_part["command"]["arguments"].as_array().cloned().unwrap_or_default();
            self.run_code_lens_command(&id.to_owned(), args, window, cx);
        }
    }

    /// `executeEdits('inlayHint.default', ...)` of the hint's text edits, then the hints are asked again.
    fn apply_inlay_hint_edits(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(item) = self.inlay_hints.items.get(id) else { return };
        let Some(edits) = item.hint["textEdits"].as_array().filter(|edits| !edits.is_empty()).cloned() else { return };
        let Some(editor) = self.editors.active_mut().filter(|editor| Some(editor.model_id) == self.inlay_hints.model_id) else { return };
        let lines = editor.document().lines.clone();
        let position = |line: &Value, column: &Value| {
            let line = (line.as_u64().unwrap_or(1).max(1) as usize - 1).min(lines.len() - 1);
            Position::new(line, char_column(&lines[line], column.as_u64().unwrap_or(1).max(1) as usize - 1))
        };
        let replaces = edits
            .iter()
            .map(|edit| {
                let range = &edit["range"];
                (position(&range["startLineNumber"], &range["startColumn"]), position(&range["endLineNumber"], &range["endColumn"]), edit["text"].as_str().unwrap_or_default().to_owned(), false)
            })
            .collect();
        let change = editor.execute_edits(replaces, None, crate::editor::EditReason::Unknown("inlayHint.default"));
        self.editor_edited(change, true, "api", cx);
        self.schedule_inlay_hints(Duration::ZERO, cx);
    }

    /// clickLinkGesture.ts `hasTriggerModifier`: Cmd, or Alt when `editor.multiCursorModifier` is `ctrlCmd`.
    fn link_modifier(&self, modifiers: Modifiers) -> bool {
        match self.setting("editor.multiCursorModifier") == json!("ctrlCmd") {
            true => modifiers.alt,
            false => modifiers.platform,
        }
    }

    /// `_executeInlayHintProvider`: every provider's hints in a range of a model, by position.
    fn execute_inlay_hint_provider(&mut self, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = super::host_requests::uri_parts(&args.first().cloned().unwrap_or_default()).ok().and_then(|(scheme, path)| self.editors.model(scheme, &path)) else {
            return Outcome::Ready(Err(crate::commands::error_value("Illegal argument")));
        };
        let uri = editor.document().uri();
        let range = args.get(1).cloned().unwrap_or_default();
        let calls: Vec<_> = self.inlay_hint_handles(editor).into_iter().map(|handle| (handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideInlayHints", vec![json!(handle), uri.clone(), range.clone()]))).collect();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let mut hints = Vec::new();
            let mut caches = Vec::new();
            for (handle, call) in calls {
                let Ok(Ok(list)) = call.await else { continue };
                if let Some(cache_id) = list["cacheId"].as_u64() {
                    caches.push((handle, cache_id));
                }
                hints.extend(list["hints"].as_array().into_iter().flatten().cloned());
            }
            let position = |hint: &Value| (hint["position"]["lineNumber"].as_u64().unwrap_or(0), hint["position"]["column"].as_u64().unwrap_or(0));
            hints.sort_by_key(position);
            let _ = sender.send(Ok(Value::Array(hints)));
            let _ = workbench.update(cx, |workbench, _| workbench.release_inlay_hint_lists(caches));
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

struct HintLayout {
    font_size: f32,
    editor_font_size: f32,
    family: String,
    padding: bool,
    maximum_length: usize,
}

fn hint_editor(editors: &crate::editor::EditorGroup, model_id: u32) -> Option<&OpenEditor> {
    editors.editors.iter().chain(&editors.models).find(|editor| editor.model_id == model_id)
}

fn hint_editor_mut(editors: &mut crate::editor::EditorGroup, model_id: u32) -> Option<&mut OpenEditor> {
    editors.editors.iter_mut().chain(&mut editors.models).find(|editor| editor.model_id == model_id)
}

fn pressed(modifiers: Modifiers) -> bool {
    modifiers.alt && modifiers.control && !(modifiers.shift || modifiers.platform)
}

/// `_getHintsRanges`: the visible lines with a viewport above and below, 30 more lines each way, merged.
fn hint_ranges(editor: &OpenEditor, viewport: crate::editor::Viewport) -> Vec<Value> {
    const EXTRA: usize = 30;
    let lines = &editor.document().lines;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (start, end) in super::semantic_tokens::visible_ranges_plus_viewport_above_below(editor, viewport) {
        let (start, end) = (start.saturating_sub(EXTRA), (end + EXTRA).min(lines.len() - 1));
        match ranges.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => ranges.push((start, end)),
        }
    }
    ranges.into_iter().map(|(start, end)| json!({ "startLineNumber": start + 1, "startColumn": 1, "endLineNumber": end + 1, "endColumn": lines[end].encode_utf16().count() + 1 })).collect()
}

/// `InlayHintsFragments`: the hint at its validated position, attached after the word (or token) it ends, else
/// before the one it starts.
// shortcut: off a word the port looks at the chars around the position, where upstream takes the token at it.
fn hint_item(lines: &[String], handle: u64, hint: Value) -> HintItem {
    let line = (hint["position"]["lineNumber"].as_u64().unwrap_or(1).max(1) as usize - 1).min(lines.len() - 1);
    let text = &lines[line];
    let column = char_column(text, hint["position"]["column"].as_u64().unwrap_or(1).max(1) as usize - 1);
    let after = match word_at(text, column) {
        Some(word) => word.start < column,
        None => column > 0 && text.chars().nth(column - 1).is_some_and(|c| !c.is_whitespace()) && text.chars().nth(column).is_none_or(char::is_whitespace),
    };
    HintItem { handle, hint, position: Position::new(line, column), after, resolved: false }
}

fn label_part(hint: &Value, index: usize) -> Value {
    match &hint["label"] {
        Value::String(label) => json!({ "label": label }),
        parts => parts[index].clone(),
    }
}

fn label_length(hint: &Value) -> usize {
    match &hint["label"] {
        Value::String(label) => label.encode_utf16().count(),
        parts => parts.as_array().into_iter().flatten().map(|part| part["label"].as_str().unwrap_or_default().encode_utf16().count()).sum(),
    }
}

/// The label parts with `maximumLength` applied over the line's running total (an ellipsis replacing the rest), and
/// a fixed length cutting or padding with hair spaces; spaces and tabs become no-break spaces (`fixSpace`).
fn rendered_parts(hint: &Value, line_total: &mut usize, maximum_length: usize, fixed_length: Option<usize>) -> Vec<RenderedPart> {
    let labels: Vec<String> = match &hint["label"] {
        Value::String(label) => vec![label.clone()],
        parts => parts.as_array().into_iter().flatten().map(|part| part["label"].as_str().unwrap_or_default().to_owned()).collect(),
    };
    let mut parts = Vec::new();
    let mut actual = 0;
    for (index, label) in labels.into_iter().enumerate() {
        let mut text: Vec<char> = label.chars().collect();
        *line_total += text.len();
        let mut too_long = false;
        let over = if maximum_length != 0 { *line_total as isize - maximum_length as isize } else { 0 };
        if over > 0 {
            text.truncate(text.len().saturating_sub(over as usize));
            text.push('\u{2026}');
            too_long = true;
        }
        actual += text.len();
        if let Some(fixed) = fixed_length
            && actual >= fixed
        {
            let over_fixed = actual - fixed;
            actual -= over_fixed;
            text.truncate(text.len().saturating_sub(1 + over_fixed));
            text.push('\u{2026}');
            too_long = true;
        }
        let text: String = text.into_iter().map(|c| if c == ' ' || c == '\t' { '\u{a0}' } else { c }).collect();
        parts.push(RenderedPart { text, index });
        if too_long {
            break;
        }
    }
    if let Some(fixed) = fixed_length
        && actual < fixed
    {
        let index = parts.last().map_or(0, |part| part.index);
        parts.push(RenderedPart { text: "\u{200a}".repeat(fixed - actual), index });
    }
    parts
}

#[cfg(test)]
mod tests;
