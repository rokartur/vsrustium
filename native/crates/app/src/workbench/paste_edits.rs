//! copyPasteController.ts, postEditWidget.ts, edit.ts and the paste half of defaultProviders.ts: extension
//! `DocumentPasteEditProvider`s prepare data on copy and turn a paste into edits; the first edit applies and, with
//! more than one, the post-paste widget switches to another (Cmd+. or its button). Paste As picks one.
//! shortcut: the data transfer carries the clipboard text and the providers' copy data only; files and the
//! clipboard's `text/uri-list` resources are not read, so the path providers never offer an edit; upgrade with a
//! clipboard reader for file URLs.

use std::collections::HashMap;

use futures::FutureExt;
use futures::future::{Shared, join_all};
use gpui::{AnyElement, Context, MouseButton, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::action_widget::{Anchor, Owner};
use super::code_actions::RowLook;
use super::inline_progress::InlineProgress;
use crate::action_list::{Item, ItemKind};
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position, Selection, utf16_column};
use crate::language_features::Candidate;

mod drop;
#[cfg(test)]
mod tests;

const REGISTER: &str = "$registerPasteEditProvider";
const TEXT: &str = "text/plain";
const URI_LIST: &str = "text/uri-list";
/// `DefaultTextPasteOrDropEditProvider`.
const TEXT_PROVIDER_ID: &str = "text";
const TEXT_KIND: &str = "text.plain";
const CONFIGURE_LABEL: &str = "Configure preferred paste action...";

/// commands.ts `configurePreferredPasteAction`, `configurePreferredDropAction`: user settings JSON at `key`.
fn configure_args(key: &str) -> Value {
    json!({ "revealSetting": { "key": key, "edit": true } })
}

type CopyData = Shared<Task<Option<Value>>>;

#[derive(Default)]
pub(super) struct PasteEdits {
    /// `_currentCopyOperation`: the handle written into the clipboard metadata and each provider's prepared data
    /// with its copy mime types.
    copy: Option<(String, Vec<(Vec<String>, CopyData)>)>,
    /// `_currentPasteOperation`.
    operation: Option<Task<()>>,
    pub(super) progress: Option<InlineProgress>,
    widget: Option<PostEditWidget>,
    /// The edits the Paste As pick shows, with the ranges they replace.
    pick: Option<(Vec<PasteEdit>, Vec<(Position, Position)>)>,
    /// What a cancelled paste falls back to: the clipboard text and its metadata.
    fallback: Option<(String, Value)>,
    /// The dotted `dnd-target` marker while files are dragged over the text with Shift.
    drop_target: Option<Position>,
}

/// `PasteEditWithProvider`: an edit and the provider that made it (`None` for the default text provider).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PasteEdit {
    handle: Option<u64>,
    title: String,
    kind: Option<String>,
    insert_text: Value,
    additional_edit: Value,
    yield_to: Vec<Value>,
    handled_mime: Option<String>,
    cache_id: Value,
}

/// `PostEditWidget` with its `EditSet`: shown below the end of the pasted range until the cursor or the text moves.
struct PostEditWidget {
    model_id: u32,
    version_id: u32,
    selections: Vec<Selection>,
    position: Position,
    /// The ranges the edits replace, as the paste found them.
    ranges: Vec<(Position, Position)>,
    edits: Vec<PasteEdit>,
    active: usize,
    /// The post-drop widget (`dropWidgetVisible`, "Show drop options...") rather than the post-paste one.
    drop: bool,
}

/// `PastePreference`.
#[derive(Clone, Debug)]
enum Preference {
    /// Paste As without arguments: every edit, picked from a list.
    Any,
    Only(String),
    Preferences(Vec<String>),
    Provider(&'static str),
}

impl Workbench {
    pub(super) fn register_paste_edits(&mut self) {
        self.commands.register("editor.changePasteType", |workbench, _, _, cx| {
            workbench.show_paste_selector(cx);
            Outcome::done()
        });
        self.commands.register("editor.hidePasteWidget", |workbench, _, _, cx| {
            workbench.clear_paste_widget(cx);
            Outcome::done()
        });
        self.commands.register("editor.changeDropType", |workbench, _, _, cx| {
            workbench.show_paste_selector(cx);
            Outcome::done()
        });
        self.commands.register("editor.hideDropWidget", |workbench, _, _, cx| {
            workbench.clear_paste_widget(cx);
            Outcome::done()
        });
        self.commands.register("editor.action.pasteAs", |workbench, args, window, cx| {
            let args = args.first().cloned().unwrap_or_default();
            let preference = match (args["kind"].as_str(), args["preferences"].as_array()) {
                (Some(kind), _) => Some(Preference::Only(kind.to_owned())),
                (None, Some(kinds)) => Some(Preference::Preferences(kinds.iter().filter_map(Value::as_str).map(str::to_owned).collect())),
                (None, None) => None,
            };
            workbench.paste_as(preference, window, cx)
        });
        self.commands.register("editor.action.pasteAsText", |workbench, _, window, cx| workbench.paste_as(Some(Preference::Provider(TEXT_PROVIDER_ID)), window, cx));
    }

    /// `pasteWidgetVisible` and `dropWidgetVisible`.
    pub(super) fn paste_edits_context(&self, context: &mut HashMap<String, Value>) {
        let drop = self.paste_edits.widget.as_ref().is_some_and(|widget| widget.drop);
        context.insert("pasteWidgetVisible".into(), json!(self.paste_widget_valid() && !drop));
        context.insert("dropWidgetVisible".into(), json!(self.paste_widget_valid() && drop));
    }

    fn paste_widget_valid(&self) -> bool {
        let Some(widget) = &self.paste_edits.widget else { return false };
        self.editors.active().is_some_and(|editor| editor.model_id == widget.model_id && editor.text_model().version_id == widget.version_id && editor.selections == widget.selections)
    }

    fn clear_paste_widget(&mut self, cx: &mut Context<Self>) {
        if self.paste_edits.widget.take().is_some() {
            cx.notify();
        }
    }

    /// `handleCopy`: the providers that prepare a paste get the copied ranges; their data waits for the next paste
    /// whose clipboard metadata carries the returned handle.
    pub(super) fn prepare_document_paste(&mut self, metadata: &mut Value, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        if !self.paste_as_enabled(editor) {
            return;
        }
        let providers: Vec<(u64, Vec<String>)> = self
            .paste_providers(editor)
            .into_iter()
            .filter(|(_, metadata)| metadata["supportsCopy"] == true)
            .map(|(handle, metadata)| (handle, strings(&metadata["copyMimeTypes"])))
            .collect();
        if providers.is_empty() {
            return;
        }
        let uri = editor.document().uri();
        let lines = &editor.document().lines;
        let ranges: Vec<Value> = editor.selections.iter().map(|selection| range_dto(lines, selection.start(), selection.end())).collect();
        let handle = exthost::main_thread_storage::generate_uuid();
        metadata["id"] = json!(handle);
        metadata["providerCopyMimeTypes"] = json!(providers.iter().flat_map(|(_, mimes)| mimes.clone()).collect::<Vec<_>>());
        let mut operations = Vec::new();
        for (provider, mimes) in providers {
            let call = self.host.call_cancellable("ExtHostLanguageFeatures", "$prepareDocumentPaste", vec![json!(provider), uri.clone(), json!(ranges), json!({ "items": [] })]);
            let data = cx.background_spawn(async move { call.await.ok().and_then(Result::ok).filter(|data| !data.is_null()) }).shared();
            operations.push((mimes, data));
        }
        self.paste_edits.copy = Some((handle, operations));
    }

    /// `handlePaste` before the default paste: `true` when the providers took the paste over.
    pub(super) fn try_paste_with_providers(&mut self, text: &str, metadata: &Value, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.paste_with_providers(text, metadata, None, window, cx)
    }

    fn paste_with_providers(&mut self, text: &str, metadata: &Value, preference: Option<Preference>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.editors.active() else { return false };
        if !self.paste_as_enabled(editor) && preference.is_none() {
            return false;
        }
        let mut potential: Vec<String> = vec![TEXT.into(), URI_LIST.into()];
        potential.extend(strings(&metadata["providerCopyMimeTypes"]));
        let providers: Vec<(u64, Value)> = self
            .paste_providers(editor)
            .into_iter()
            .filter(|(_, provider)| preference.as_ref().is_none_or(|preference| provider_matches(provider, preference)))
            .filter(|(_, provider)| strings(&provider["pasteMimeTypes"]).iter().any(|pattern| potential.iter().any(|mime| matches_mime(pattern, mime))))
            .collect();
        let text_provider = preference.as_ref().is_none_or(|preference| !matches!(preference, Preference::Provider(id) if *id != TEXT_PROVIDER_ID));
        if providers.is_empty() && preference.is_none() {
            return false;
        }
        let copy_data = self.copy_data_for(metadata, &providers);
        let (model_id, version_id, selections) = (editor.model_id, editor.text_model().version_id, editor.selections.clone());
        let uri = editor.document().uri();
        let ranges: Vec<(Position, Position)> = selections.iter().map(|selection| (selection.start(), selection.end())).collect();
        let range_dtos: Vec<Value> = ranges.iter().map(|(start, end)| range_dto(&editor.document().lines, *start, *end)).collect();
        self.clear_paste_widget(cx);
        let text = text.to_owned();
        let paste_as = preference.is_some();
        let trigger_kind = if paste_as { 1 } else { 0 };
        let only = match &preference {
            Some(Preference::Only(kind)) => json!(kind),
            _ => Value::Null,
        };
        let end = selections[0].end();
        self.paste_edits.fallback = Some((text.clone(), metadata.clone()));
        let operation = cx.spawn_in(window, async move |workbench, cx| {
            let mut items = vec![json!([TEXT, { "id": "0", "asString": text }])];
            for data in join_all(copy_data).await.into_iter().rev().flatten() {
                for item in data["items"].as_array().into_iter().flatten() {
                    items.retain(|known| known[0] != item[0]);
                    items.push(item.clone());
                }
            }
            let transfer_mimes: Vec<String> = items.iter().filter_map(|item| item[0].as_str().map(str::to_owned)).collect();
            let supported: Vec<(u64, Value)> = providers.into_iter().filter(|(_, provider)| strings(&provider["pasteMimeTypes"]).iter().any(|pattern| transfer_mimes.iter().any(|mime| matches_mime(pattern, mime)))).collect();
            let context = json!({ "only": only, "triggerKind": trigger_kind });
            let Ok(calls) = workbench.update(cx, |workbench, _| {
                let mut calls = Vec::new();
                for (request_id, (handle, _)) in supported.iter().enumerate() {
                    let args = vec![json!(handle), json!(request_id), uri.clone(), json!(range_dtos), json!({ "items": items }), context.clone()];
                    calls.push(workbench.host.call_cancellable("ExtHostLanguageFeatures", "$providePasteEdits", args));
                }
                calls
            }) else {
                return;
            };
            let answers: Vec<Value> = join_all(calls).await.into_iter().map(|answer| answer.ok().and_then(Result::ok).unwrap_or(Value::Null)).collect();
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.clear_inline_progress(cx);
                let mut edits = Vec::new();
                for ((handle, _), answer) in supported.iter().zip(&answers) {
                    edits.extend(answer.as_array().into_iter().flatten().map(|dto| paste_edit(Some(*handle), dto)));
                }
                if text_provider {
                    edits.push(default_text_edit(&text));
                }
                edits = filter_by_preference(sort_by_yield_to(edits), preference.as_ref());
                let still = workbench.editors.active().is_some_and(|editor| editor.model_id == model_id && editor.text_model().version_id == version_id && editor.selections == selections);
                if !still {
                    return;
                }
                workbench.finish_paste(edits, ranges, preference, window, cx);
            });
        });
        self.paste_edits.operation = Some(operation);
        // Paste As reports its progress in the window (`ProgressLocation.Window`), not inline.
        if !paste_as {
            self.show_inline_progress(model_id, end, "Running paste handlers. Click to cancel and do basic paste".into(), cx);
        }
        true
    }

    /// The edits are in: Paste As shows a pick (or takes the first for a preference), a paste applies the
    /// preferred edit unless only the default text edit is left.
    fn finish_paste(&mut self, edits: Vec<PasteEdit>, ranges: Vec<(Position, Position)>, preference: Option<Preference>, window: &mut Window, cx: &mut Context<Self>) {
        let fallback = self.paste_edits.fallback.take();
        self.paste_edits.operation = None;
        if matches!(preference, Some(Preference::Any)) && !edits.is_empty() {
            return self.show_paste_as_pick(edits, ranges, window, cx);
        }
        if let Some(preference) = &preference {
            let Some(edit) = edits.into_iter().next() else {
                let kind = match preference {
                    Preference::Only(kind) => kind.clone(),
                    Preference::Preferences(kinds) if kinds.is_empty() => "empty".into(),
                    Preference::Preferences(kinds) => kinds.join(", "),
                    Preference::Provider(id) => (*id).to_owned(),
                    Preference::Any => return,
                };
                self.show_editor_message(vec![super::editor_message::MessagePart::text(format!("No paste edits for '{kind}' found"))], cx);
                return;
            };
            return self.apply_paste_edit(edit, &ranges, cx);
        }
        let only_text = edits.is_empty() || (edits.len() == 1 && edits[0].handle.is_none());
        if only_text {
            if let Some((text, metadata)) = fallback {
                drop(self.default_paste(&text, &metadata, cx));
            }
            return;
        }
        let active = self.initial_paste_edit_index(&edits);
        let editor = self.editors.active().expect("checked by the caller");
        let show_widget = self.editor_string_setting(editor, "editor.pasteAs.showPasteSelector") != "never";
        let edit = edits[active].clone();
        if show_widget && edits.len() > 1 {
            self.show_paste_widget(edits, active, ranges.clone(), false);
        }
        self.apply_paste_edit(edit, &ranges, cx);
    }

    /// `showPasteAsPick`: the edits with their kinds, then "Configure preferred paste action...".
    fn show_paste_as_pick(&mut self, edits: Vec<PasteEdit>, ranges: Vec<(Position, Position)>, window: &mut Window, cx: &mut Context<Self>) {
        use super::editor_status::StatusPick;
        use crate::quick_input::{Entry, Kind, QuickInput};
        let mut rows = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            let mut item = super::editor_status::label_item(&edit.title);
            item.description = edit.kind.clone();
            rows.push((Entry::Item(item), StatusPick::PasteEdit(index)));
        }
        rows.push((Entry::Separator(None), StatusPick::Nothing));
        let configure = configure_args("editor.pasteAs.preferences");
        rows.push((Entry::Item(super::editor_status::label_item(CONFIGURE_LABEL)), StatusPick::ExtensionCommand("workbench.action.openSettingsJson".into(), vec![configure])));
        self.paste_edits.pick = Some((edits, ranges));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Paste Action".into());
        self.show_status_picker(input, rows, window, cx);
    }

    /// The Paste As pick's edit applies.
    pub(super) fn accept_paste_as_pick(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((edits, ranges)) = self.paste_edits.pick.take() else { return };
        self.apply_paste_edit(edits[index].clone(), &ranges, cx);
    }

    /// `getInitialActiveEditIndex`: the first edit of the first `editor.pasteAs.preferences` kind that has one.
    fn initial_paste_edit_index(&self, edits: &[PasteEdit]) -> usize {
        let editor = self.editors.active().expect("an active editor pastes");
        let preferences = self.editor_setting_value(editor, "editor.pasteAs.preferences");
        for kind in preferences.as_array().into_iter().flatten().filter_map(Value::as_str) {
            if let Some(index) = edits.iter().position(|edit| edit.kind.as_deref().is_some_and(|edit_kind| kind_contains(kind, edit_kind))) {
                return index;
            }
        }
        0
    }

    /// `applyEditAndShowIfNeeded` after the resolve: `createCombinedWorkspaceEdit` through the bulk edit service.
    fn apply_paste_edit(&mut self, edit: PasteEdit, ranges: &[(Position, Position)], cx: &mut Context<Self>) {
        let supports_resolve = edit.handle.and_then(|handle| self.language_features.registration(handle)).is_some_and(|registration| registration.args[2]["supportsResolve"] == true);
        let resolve = (supports_resolve && !edit.cache_id.is_null()).then(|| self.host.call_cancellable("ExtHostLanguageFeatures", "$resolvePasteEdit", vec![json!(edit.handle), edit.cache_id.clone()]));
        let Some(resolve) = resolve else {
            return self.apply_resolved_paste_edit(&edit, ranges, cx);
        };
        let ranges = ranges.to_vec();
        cx.spawn(async move |workbench, cx| {
            let mut edit = edit;
            if let Ok(Ok(resolved)) = resolve.await {
                if !resolved["insertText"].is_null() {
                    edit.insert_text = resolved["insertText"].clone();
                }
                if !resolved["additionalEdit"].is_null() {
                    edit.additional_edit = resolved["additionalEdit"].clone();
                }
            }
            let _ = workbench.update(cx, |workbench, cx| workbench.apply_resolved_paste_edit(&edit, &ranges, cx));
        })
        .detach();
    }

    fn apply_resolved_paste_edit(&mut self, edit: &PasteEdit, ranges: &[(Position, Position)], cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let length_before = document_length(&editor.document().lines);
        let edits = combined_workspace_edit(editor, ranges, edit);
        if let Err(error) = self.apply_workspace_text_edits(&Value::Array(edits), &Value::Null, files::operations::new_undo_group(), cx) {
            self.notify_error(format!("Error applying edit '{}':\n{}", edit.title, error["message"].as_str().unwrap_or_default()));
        }
        if let Some(widget) = self.paste_edits.widget.as_mut()
            && let Some(editor) = self.editors.active()
        {
            widget.version_id = editor.text_model().version_id;
            widget.selections = editor.selections.clone();
            // The `paste-line-suffix` decoration grows over the primary range's edit; the widget sits at its end.
            let lines = &editor.document().lines;
            let grown = document_length(lines) as isize - length_before as isize;
            widget.position = position_of(lines, (offset_of(lines, ranges[0].1) as isize + grown).max(0) as usize);
        }
        cx.notify();
    }

    fn show_paste_widget(&mut self, edits: Vec<PasteEdit>, active: usize, ranges: Vec<(Position, Position)>, drop: bool) {
        let editor = self.editors.active().expect("an active editor pastes");
        let (model_id, version_id, selections, position) = (editor.model_id, editor.text_model().version_id, editor.selections.clone(), editor.selection().end());
        self.paste_edits.widget = Some(PostEditWidget { model_id, version_id, selections, position, ranges, edits, active, drop });
    }

    /// `showSelector`: the edits in the action widget, the active one checked, below the widget's button.
    fn show_paste_selector(&mut self, cx: &mut Context<Self>) {
        if !self.paste_widget_valid() {
            return;
        }
        let widget = self.paste_edits.widget.as_ref().expect("checked above");
        let mut items = Vec::new();
        let mut looks = Vec::new();
        for (index, edit) in widget.edits.iter().enumerate() {
            let mut item = Item::new(ItemKind::Action);
            item.item = Some(index);
            item.label = Some(edit.title.clone());
            items.push(item);
            looks.push(RowLook { icon: if index == widget.active { "check" } else { "blank" }, color: None, keybinding: None });
        }
        let anchor = Anchor::Position(Position::new(widget.position.line + 1, widget.position.column + 1));
        let (label, key) = if widget.drop { ("Configure preferred drop action...", "editor.dropIntoEditor.preferences") } else { (CONFIGURE_LABEL, "editor.pasteAs.preferences") };
        let configure = json!({ "id": "workbench.action.openSettingsJson", "arguments": [configure_args(key)] });
        self.show_action_widget(items, looks, anchor, vec![(label.into(), Some(configure))], Owner::PasteEdits, cx);
    }

    /// `onSelectNewEdit`: the applied edit is undone and the picked one applies with the widget kept.
    pub(super) fn select_paste_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.editor_focus, cx);
        if !self.paste_widget_valid() {
            return;
        }
        let widget = self.paste_edits.widget.as_mut().expect("checked above");
        if index == widget.active {
            return;
        }
        widget.active = index;
        let (edit, ranges) = (widget.edits[index].clone(), widget.ranges.clone());
        drop(self.execute_command("undo", Vec::new(), window, cx));
        self.apply_paste_edit(edit, &ranges, cx);
    }

    /// The inline progress' click: the providers' answers are dropped and the basic paste runs.
    pub(super) fn cancel_paste(&mut self, cx: &mut Context<Self>) {
        self.paste_edits.operation = None;
        self.clear_inline_progress(cx);
        if let Some((text, metadata)) = self.paste_edits.fallback.take() {
            drop(self.default_paste(&text, &metadata, cx));
        }
    }

    /// `pasteAs`: the clipboard through the providers that match `preference`, picking an edit.
    fn paste_as(&mut self, preference: Option<Preference>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.edit_blocked(true, cx) {
            return Outcome::done();
        }
        window.focus(&self.editor_focus, cx);
        let Some(gpui::ClipboardEntry::String(clipboard)) = cx.read_from_clipboard().and_then(|item| item.entries().first().cloned()) else { return Outcome::done() };
        let metadata = clipboard.metadata_json::<Value>().unwrap_or_default();
        let preference = preference.unwrap_or(Preference::Any);
        self.paste_with_providers(&clipboard.text(), &metadata, Some(preference), window, cx);
        Outcome::done()
    }

    /// The `$registerPasteEditProvider` providers of the editor's model with their metadata, best first.
    fn paste_providers(&self, editor: &OpenEditor) -> Vec<(u64, Value)> {
        let uri = editor.document().uri();
        let candidate = Candidate { uri: &uri, language: &editor.document().language_id };
        let providers = self.language_features.ordered(REGISTER, &candidate);
        providers.iter().filter(|registration| registration.args[2]["supportsPaste"] == true || registration.args[2]["supportsCopy"] == true).map(|registration| (registration.handle, registration.args[2].clone())).collect()
    }

    /// `mergeInDataFromCopy`: the copy's prepared data when the clipboard carries its handle, of the providers whose
    /// copy mimes a paste provider takes.
    fn copy_data_for(&self, metadata: &Value, providers: &[(u64, Value)]) -> Vec<CopyData> {
        let Some((handle, operations)) = &self.paste_edits.copy else { return Vec::new() };
        if metadata["id"].as_str() != Some(handle) {
            return Vec::new();
        }
        let wanted = |mimes: &Vec<String>| providers.iter().any(|(_, provider)| strings(&provider["pasteMimeTypes"]).iter().any(|pattern| mimes.iter().any(|mime| matches_mime(pattern, mime))));
        operations.iter().filter(|(mimes, _)| wanted(mimes)).map(|(_, data)| data.clone()).collect()
    }

    fn paste_as_enabled(&self, editor: &OpenEditor) -> bool {
        self.editor_setting_value(editor, "editor.pasteAs.enabled") != json!(false)
    }

    fn editor_string_setting(&self, editor: &OpenEditor, key: &str) -> String {
        self.editor_setting_value(editor, key).as_str().unwrap_or_default().to_owned()
    }

    fn editor_setting_value(&self, editor: &OpenEditor, key: &str) -> Value {
        let document = editor.document();
        let overrides = config::configuration::Overrides { resource: Some(&document.path), override_identifier: Some(&document.language_id) };
        self.settings.lock().unwrap().get_value(Some(key), &overrides).unwrap_or_default()
    }

    /// `PostEditWidget`: the `insert` button below the end of the pasted text.
    pub(super) fn paste_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.paste_edits.widget.as_ref().filter(|widget| widget.model_id == editor.model_id)?;
        if !self.paste_widget_valid() {
            return None;
        }
        let viewport = self.editors.viewport;
        let view_line = editor.position_view_line(widget.position, false);
        let segment = editor.segment(view_line);
        let left = editor.segment_visible_column(segment, widget.position.column) * char_width - editor.scroll.left + content_left;
        let top = editor.line_top(view_line, viewport) - editor.scroll.top + viewport.line_height;
        let button = self
            .icon("insert", 22., 16., self.color("button.foreground"))
            .id("post-edit-widget")
            .absolute()
            .left(px(left))
            .top(px(top))
            .rounded(px(2.))
            .bg(self.color("button.background"))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("button.hoverBackground")))
            .child(self.hover_target("post-edit-widget-hover", super::hover::HoverContent::Text(if widget.drop { "Show drop options... (\u{2318}.)" } else { "Show paste options... (\u{2318}.)" }.into()), super::hover::HoverOptions::mouse()))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, _, cx| {
                cx.stop_propagation();
                workbench.show_paste_selector(cx);
            }));
        Some(div().child(button).into_any_element())
    }
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// dataTransfer.ts `matchesMimeType`: `*/*`, `type/*` or the exact type, ignoring case.
fn matches_mime(pattern: &str, mime: &str) -> bool {
    let (pattern, mime) = (pattern.to_lowercase(), mime.to_lowercase());
    if pattern == "*/*" || pattern == mime {
        return true;
    }
    pattern.strip_suffix("/*").is_some_and(|prefix| mime.split('/').next() == Some(prefix))
}

/// `HierarchicalKind.contains`: the kind itself or one below it.
fn kind_contains(kind: &str, other: &str) -> bool {
    kind.is_empty() || other == kind || other.starts_with(&format!("{kind}."))
}

fn document_length(lines: &[String]) -> usize {
    lines.iter().map(|line| line.chars().count() + 1).sum::<usize>().saturating_sub(1)
}

fn offset_of(lines: &[String], position: Position) -> usize {
    lines[..position.line].iter().map(|line| line.chars().count() + 1).sum::<usize>() + position.column
}

fn position_of(lines: &[String], mut offset: usize) -> Position {
    for (line, text) in lines.iter().enumerate() {
        let length = text.chars().count();
        if offset <= length {
            return Position::new(line, offset);
        }
        offset -= length + 1;
    }
    Position::new(lines.len() - 1, lines[lines.len() - 1].chars().count())
}

/// `providerMatchesPreference`.
fn provider_matches(provider: &Value, preference: &Preference) -> bool {
    let kinds = strings(&provider["providedPasteEditKinds"]);
    match preference {
        Preference::Any => true,
        Preference::Only(only) => kinds.iter().any(|kind| kind_contains(only, kind)),
        Preference::Preferences(preferences) => kinds.iter().any(|kind| preferences.iter().any(|preferred| kind_contains(preferred, kind))),
        Preference::Provider(_) => false,
    }
}

/// The edits Paste As keeps for its preference.
fn filter_by_preference(edits: Vec<PasteEdit>, preference: Option<&Preference>) -> Vec<PasteEdit> {
    let Some(preference) = preference else { return edits };
    edits
        .into_iter()
        .filter(|edit| match preference {
            Preference::Any => true,
            Preference::Only(only) => edit.kind.as_deref().is_some_and(|kind| kind_contains(only, kind)),
            Preference::Preferences(preferences) => edit.kind.as_deref().is_some_and(|kind| preferences.iter().any(|preferred| kind_contains(preferred, kind))),
            Preference::Provider(id) => *id == TEXT_PROVIDER_ID && edit.handle.is_none(),
        })
        .collect()
}

fn paste_edit(handle: Option<u64>, dto: &Value) -> PasteEdit {
    PasteEdit {
        handle,
        title: dto["title"].as_str().unwrap_or_default().to_owned(),
        kind: dto["kind"]["value"].as_str().or(dto["kind"].as_str()).map(str::to_owned),
        insert_text: dto["insertText"].clone(),
        additional_edit: dto["additionalEdit"].clone(),
        yield_to: dto["yieldTo"].as_array().cloned().unwrap_or_default(),
        handled_mime: dto["handledMimeType"].as_str().map(str::to_owned),
        cache_id: dto["_cacheId"].clone(),
    }
}

/// `DefaultTextPasteOrDropEditProvider.getEdit`.
fn default_text_edit(text: &str) -> PasteEdit {
    PasteEdit {
        handle: None,
        title: "Insert Plain Text".into(),
        kind: Some(TEXT_KIND.into()),
        insert_text: json!(text),
        additional_edit: Value::Null,
        yield_to: Vec::new(),
        handled_mime: Some(TEXT.into()),
        cache_id: Value::Null,
    }
}

/// edit.ts `sortEditsByYieldTo`: an edit comes after every edit it yields to (by kind or handled mime type).
fn sort_by_yield_to(edits: Vec<PasteEdit>) -> Vec<PasteEdit> {
    let yields_to = |edit: &PasteEdit, other: &PasteEdit| {
        edit.yield_to.iter().any(|target| match (target["mimeType"].as_str(), target["kind"].as_str().or(target["kind"]["value"].as_str())) {
            (Some(mime), _) => other.handled_mime.as_deref() == Some(mime),
            (None, Some(kind)) => other.kind.as_deref().is_some_and(|other_kind| kind_contains(kind, other_kind)),
            (None, None) => false,
        })
    };
    let count = edits.len();
    let before: Vec<Vec<usize>> = (0..count).map(|index| (0..count).filter(|&other| other != index && yields_to(&edits[index], &edits[other])).collect()).collect();
    if before.iter().all(Vec::is_empty) {
        return edits;
    }
    let mut order = Vec::new();
    let mut visited = vec![false; count];
    let mut stack = Vec::new();
    for index in 0..count {
        visit(index, &before, &mut visited, &mut stack, &mut order);
    }
    order.into_iter().map(|index| edits[index].clone()).collect()
}

fn visit(index: usize, before: &[Vec<usize>], visited: &mut [bool], stack: &mut Vec<usize>, order: &mut Vec<usize>) {
    if visited[index] || stack.contains(&index) {
        return;
    }
    stack.push(index);
    for &other in &before[index] {
        visit(other, before, visited, stack, order);
    }
    stack.pop();
    visited[index] = true;
    order.push(index);
}

/// edit.ts `createCombinedWorkspaceEdit`: the insert text as a snippet at every range (plain text escaped with `$0`
/// after it), then the additional edit's edits; an empty insert text keeps only the additional edits.
fn combined_workspace_edit(editor: &OpenEditor, ranges: &[(Position, Position)], edit: &PasteEdit) -> Vec<Value> {
    let snippet = match &edit.insert_text {
        Value::String(text) => (!text.is_empty()).then(|| format!("{}$0", super::bulk_text_edits::escape_snippet(text))),
        other => other["snippet"].as_str().filter(|snippet| !snippet.is_empty()).map(str::to_owned),
    };
    let mut edits = Vec::new();
    if let Some(snippet) = snippet {
        let lines = &editor.document().lines;
        let (uri, version) = (editor.document().uri(), editor.text_model().version_id);
        for (start, end) in ranges {
            edits.push(json!({ "resource": uri, "versionId": version, "textEdit": { "range": range_dto(lines, *start, *end), "text": snippet, "insertAsSnippet": true } }));
        }
    }
    edits.extend(edit.additional_edit["edits"].as_array().into_iter().flatten().filter(|edit| edit.get("textEdit").is_some()).cloned());
    edits
}

fn range_dto(lines: &[String], start: Position, end: Position) -> Value {
    let column = |position: Position| utf16_column(&lines[position.line], position.column) + 1;
    json!({ "startLineNumber": start.line + 1, "startColumn": column(start), "endLineNumber": end.line + 1, "endColumn": column(end) })
}
