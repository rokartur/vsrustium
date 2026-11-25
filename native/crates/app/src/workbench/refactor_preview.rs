//! The Refactor Preview (contrib/bulkEdit/browser/preview: bulkEdit.contribution.ts, bulkEditPane.ts,
//! bulkEditTree.ts): a workspace edit that needs confirmation, or one asked to preview, waits in the panel's
//! `refactorPreview` view with a checkbox per category, file and text edit until Apply or Discard; the host then
//! applies the checked edits.
// Not ported yet: the multi-file diff editor upstream opens an element in (milestone 14's; an element opens a diff
// editor of its file instead, and create, delete and rename elements open none), the conflict check while the
// preview is open (a text edit of a changed model still fails on its version), keyboard navigation of the tree.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use futures::channel::oneshot;
use gpui::{AnyElement, ClickEvent, Context, Div, MouseButton, MouseDownEvent, PromptLevel, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::bulk_text_edits::document_end;
use super::panel::PanelView;
use crate::bulk_edit_preview::{CREATE, Category, DELETE, FileOperation, Preview, RENAME, TEXT_EDIT, apply_text_edits, text_edit_parts};
use crate::commands::Outcome;
use crate::diff_editor::Side;
use crate::editor::{EditReason, Position, TextDocument};
use crate::menus::MenuAction;

/// `BulkEditPane.ID`, the container's and the view's.
pub const REFACTOR_PREVIEW_ID: &str = "refactorPreview";
/// `BulkEditPreviewProvider.Schema`.
const PREVIEW_SCHEME: &str = "vscode-bulkeditpreview-editor";
const GROUP_BY_FILE_KEY: &str = "refactorPreview.groupByFile";
/// bulkEditTree.ts `BulkEditDelegate.ItemHeight`.
const ROW_HEIGHT: f32 = 23.;

#[derive(Default)]
pub struct RefactorPreview {
    preview: Option<Preview>,
    reply: Option<oneshot::Sender<Result<Value, Value>>>,
    /// `UXState`: the panel view shown before the preview, `None` when the panel was hidden.
    previous_panel: Option<Option<&'static str>>,
    group_by_file: Option<bool>,
    expanded: HashSet<String>,
}

/// A row of the tree: a category, a file (its category when grouped by type) or a text edit of a file.
#[derive(Clone)]
enum Row {
    Category(usize),
    File(Option<usize>, FileOperation),
    TextEdit(Option<usize>, FileOperation, usize),
}

impl Workbench {
    pub(super) fn register_refactor_preview(&mut self) {
        self.register_panel_view(PanelView {
            id: REFACTOR_PREVIEW_ID,
            view_id: REFACTOR_PREVIEW_ID,
            title: "Refactor Preview",
            open: |workbench, _, cx| workbench.open_panel_view(REFACTOR_PREVIEW_ID, cx),
            render: |workbench, _, cx| workbench.refactor_preview_body(cx).into_any_element(),
            actions: |workbench, _, cx| workbench.refactor_preview_title_actions(cx).into_any_element(),
            // `hideIfEmpty` with the view's `when: refactorPreview.enabled`.
            visible: |workbench| workbench.refactor_preview.preview.is_some(),
            badge: |_| None,
        });
        self.commands.register("refactorPreview.apply", |workbench, _, window, cx| {
            if workbench.refactor_preview.preview.as_ref().is_some_and(Preview::has_checked_changes) {
                workbench.finish_refactor_preview(true, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("refactorPreview.discard", |workbench, _, window, cx| {
            workbench.finish_refactor_preview(false, window, cx);
            Outcome::done()
        });
        for (id, group_by_file) in [("refactorPreview.groupByFile", Some(true)), ("refactorPreview.groupByType", Some(false)), ("refactorPreview.toggleGrouping", None)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                let value = group_by_file.unwrap_or(!workbench.refactor_preview_group_by_file());
                workbench.refactor_preview.group_by_file = Some(value);
                workbench.profile_storage.lock().unwrap().set(GROUP_BY_FILE_KEY, value.to_string());
                cx.notify();
                Outcome::done()
            });
        }
    }

    /// bulkEditService.ts `apply` with a preview handler: `edits` of a workspace edit DTO wait in the view; the
    /// reply is the DTO with the checked edits, none on Discard.
    pub fn preview_workspace_edit(&mut self, dto: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        // Extension tests show no dialogs or previews.
        if self.skip_dialogs {
            return Outcome::Ready(Ok(dto.clone()));
        }
        let (sender, receiver) = oneshot::channel();
        let dto = dto.clone();
        if self.refactor_preview.preview.is_none() {
            self.show_refactor_preview(dto, sender, cx);
            return Outcome::Pending(receiver);
        }
        // `_previewEdit`: another preview asks before it replaces this one.
        let answer = crate::dialog::show_message_box(
            window,
            PromptLevel::Info,
            "Another refactoring is being previewed.",
            Some("Press 'Continue' to discard the previous refactoring and continue with the current refactoring."),
            &["Continue", "Cancel"],
            None,
            cx,
        );
        cx.spawn_in(window, async move |workbench, cx| {
            let proceed = matches!(answer.await, Ok((0, _)));
            let _ = workbench.update(cx, |workbench, cx| match proceed {
                true => {
                    if let Some(reply) = workbench.refactor_preview.reply.take() {
                        let _ = reply.send(Ok(json!({ "edits": [] })));
                    }
                    workbench.show_refactor_preview(dto, sender, cx);
                }
                false => drop(sender.send(Ok(json!({ "edits": [] })))),
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }

    fn show_refactor_preview(&mut self, dto: Value, reply: oneshot::Sender<Result<Value, Value>>, cx: &mut Context<Self>) {
        let edits = dto["edits"].as_array().cloned().unwrap_or_default();
        let exists = |uri: &Value| uri["scheme"] == "file" && uri["path"].as_str().is_some_and(|path| Path::new(path).exists());
        let preview = Preview::new(edits, exists);
        // `setInput`: the first ten files of the tree expanded.
        self.refactor_preview.expanded = preview.files.iter().take(10).map(|file| file.key.clone()).collect();
        if self.refactor_preview.previous_panel.is_none() {
            self.refactor_preview.previous_panel = Some(self.active_panel_id());
        }
        self.refactor_preview.preview = Some(preview);
        self.refactor_preview.reply = Some(reply);
        self.open_panel_view(REFACTOR_PREVIEW_ID, cx);
        cx.notify();
    }

    /// `accept` and `discard`, then `UXState.restore`: the panel's earlier view again or the panel hidden, and the
    /// preview editors closed.
    fn finish_refactor_preview(&mut self, apply: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.refactor_preview.preview.take() else { return };
        let edits = if apply { preview.checked_edits() } else { Vec::new() };
        if let Some(reply) = self.refactor_preview.reply.take() {
            let _ = reply.send(Ok(json!({ "edits": edits })));
        }
        match self.refactor_preview.previous_panel.take() {
            Some(Some(id)) => self.open_panel_view(id, cx),
            _ => self.hide_panel(cx),
        }
        let previews: Vec<usize> = self.editors.editors.iter().enumerate().filter(|(_, editor)| self.editors.diffs.get(&editor.model_id).is_some_and(|diff| diff.modified.scheme == PREVIEW_SCHEME)).map(|(index, _)| index).collect();
        if !previews.is_empty() {
            drop(self.close_editors(previews, window, cx));
        }
        cx.notify();
    }

    /// The `refactorPreview.*` context keys.
    pub(super) fn refactor_preview_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        let preview = self.refactor_preview.preview.as_ref();
        context.insert("refactorPreview.enabled".into(), json!(preview.is_some()));
        context.insert("refactorPreview.hasCategories".into(), json!(preview.is_some_and(|preview| preview.categories.len() > 1)));
        context.insert("refactorPreview.groupByFile".into(), json!(self.refactor_preview_group_by_file()));
        context.insert("refactorPreview.hasCheckedChanges".into(), json!(preview.is_none_or(Preview::has_checked_changes)));
    }

    /// `refactorPreview.groupByFile` in profile storage, by file at first; one category always groups by file.
    fn refactor_preview_group_by_file(&self) -> bool {
        let stored = self.refactor_preview.group_by_file.unwrap_or_else(|| self.profile_storage.lock().unwrap().get(GROUP_BY_FILE_KEY) != Some("false"));
        stored || self.refactor_preview.preview.as_ref().is_none_or(|preview| preview.categories.len() <= 1)
    }

    /// `MenuId.BulkEditTitle`: Group Changes By Type or By File while there are categories.
    fn refactor_preview_title_actions(&self, cx: &mut Context<Self>) -> Div {
        let has_categories = self.refactor_preview.preview.as_ref().is_some_and(|preview| preview.categories.len() > 1);
        let action = match self.refactor_preview_group_by_file() {
            true => self.search_action("ungroup-by-ref-type", "refactorPreview.groupByType", true, cx),
            false => self.search_action("group-by-ref-type", "refactorPreview.groupByFile", true, cx),
        };
        div().flex().when(has_categories, |actions| actions.child(action))
    }

    /// The message without a preview, else the tree over the Apply and Discard buttons.
    fn refactor_preview_body(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(preview) = self.refactor_preview.preview.as_ref() else {
            return div().px(px(20.)).py(px(10.)).child("Invoke a code action, like rename, to see a preview of its changes here.");
        };
        let group_by_file = self.refactor_preview_group_by_file();
        let mut rows = Vec::new();
        let push_file = |rows: &mut Vec<(Row, usize)>, category: Option<usize>, file: &FileOperation, depth: usize| {
            rows.push((Row::File(category, file.clone()), depth));
            let key = row_key(category, &file.key);
            if self.refactor_preview.expanded.contains(&key) {
                let mut text_edits = file.text_edits.clone();
                text_edits.sort_by_key(|&index| range_start(&preview.edits[index]));
                rows.extend(text_edits.into_iter().map(|index| (Row::TextEdit(category, file.clone(), index), depth + 1)));
            }
        };
        if group_by_file {
            for file in &preview.files {
                push_file(&mut rows, None, file, 0);
            }
        } else {
            for (index, category) in preview.categories.iter().enumerate() {
                rows.push((Row::Category(index), 0));
                if self.refactor_preview.expanded.contains(&format!("category:{index}")) || !self.refactor_preview.expanded.contains(&format!("category-collapsed:{index}")) {
                    for file in &category.files {
                        push_file(&mut rows, Some(index), file, 1);
                    }
                }
            }
        }
        let elements: Vec<AnyElement> = rows.into_iter().enumerate().map(|(index, (row, depth))| self.refactor_preview_row(index, row, depth, cx)).collect();
        let button = |id: &'static str, label: &'static str, primary: bool, command: &'static str| {
            let (background, hover, foreground) = match primary {
                true => (self.color("button.background"), self.color("button.hoverBackground"), self.color("button.foreground")),
                false => (self.color("button.secondaryBackground"), self.color("button.secondaryHoverBackground"), self.color("button.secondaryForeground")),
            };
            // button.css `.monaco-text-button`: 4px 8px padding, 16px line height, 12px text, `button.border`.
            div()
                .id(id)
                .mx(px(4.))
                .px(px(8.))
                .py(px(4.))
                .line_height(px(16.))
                .text_size(px(12.))
                .border_1()
                .border_color(self.theme.color("button.border").map_or(gpui::transparent_black(), Into::into))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .bg(background)
                .text_color(foreground)
                .hover(move |button| button.bg(hover))
                .child(label)
                .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx))))
        };
        let buttons = div().flex_none().h(px(50.)).pl(px(20.)).pt(px(10.)).flex().items_start().child(button("refactor-preview-apply", "Apply", true, "refactorPreview.apply")).child(button("refactor-preview-discard", "Discard", false, "refactorPreview.discard"));
        div().flex_1().min_h_0().flex().flex_col().child(div().id("refactor-preview-tree").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().children(elements)).child(buttons)
    }

    fn refactor_preview_row(&self, index: usize, row: Row, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let preview = self.refactor_preview.preview.as_ref().expect("rows draw with a preview");
        let foreground = self.foreground_or("foreground");
        let (key, checked, disabled, expandable, content): (String, Option<bool>, bool, bool, Div) = match &row {
            Row::Category(category) => {
                let category_data = &preview.categories[*category];
                (format!("category:{category}"), None, false, true, self.refactor_preview_category(category_data, foreground))
            }
            Row::File(category, file) => {
                let in_category = category.is_some();
                (row_key(*category, &file.key), Some(preview.file_checked(file, in_category)), preview.file_disabled(file, in_category), !file.text_edits.is_empty(), self.refactor_preview_file(file, foreground))
            }
            Row::TextEdit(category, file, edit) => (format!("text:{edit}"), Some(preview.checked[*edit]), preview.file_disabled(file, category.is_some()), false, self.refactor_preview_text_edit(file, *edit)),
        };
        let expanded = match &row {
            Row::Category(category) => !self.refactor_preview.expanded.contains(&format!("category-collapsed:{category}")),
            _ => self.refactor_preview.expanded.contains(&key),
        };
        let twistie = match expandable {
            true => self.icon(if expanded { "chevron-down" } else { "chevron-right" }, 16., 16., foreground),
            false => div().size(px(16.)).flex_none(),
        };
        let toggled_key = key.clone();
        let toggled_row = row.clone();
        let opened_row = row.clone();
        div()
            .id(("refactor-preview-row", index))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .mx(px(4.))
            .pl(px(4. + 8. * depth as f32))
            .rounded(px(4.))
            .cursor_pointer()
            .whitespace_nowrap()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .child(twistie.id(("refactor-preview-twistie", index)).on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                let expanded = &mut workbench.refactor_preview.expanded;
                let key = match toggled_key.strip_prefix("category:") {
                    Some(category) => format!("category-collapsed:{category}"),
                    None => toggled_key.clone(),
                };
                if !expanded.remove(&key) {
                    expanded.insert(key);
                }
                cx.notify();
            })))
            .children(checked.map(|checked| {
                self.checkbox(checked, false).id(("refactor-preview-check", index)).mr(px(4.)).when(disabled, |checkbox| checkbox.opacity(0.4)).when(!disabled, |checkbox| {
                    checkbox.on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        workbench.toggle_refactor_preview_row(&toggled_row);
                        cx.notify();
                    }))
                })
            }))
            .child(content.min_w_0().overflow_hidden())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.open_refactor_preview_row(&opened_row, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                // `MenuId.BulkEditContext`.
                let actions = vec![
                    MenuAction::Command { id: "refactorPreview.apply".into(), title: "Apply Refactoring".into(), enabled: workbench.refactor_preview.preview.as_ref().is_some_and(Preview::has_checked_changes), checked: false, alt: None },
                    MenuAction::Command { id: "refactorPreview.discard".into(), title: "Discard Refactoring".into(), enabled: true, checked: false, alt: None },
                    MenuAction::Command { id: "refactorPreview.toggleGrouping".into(), title: "Group Changes By Type".into(), enabled: workbench.refactor_preview.preview.as_ref().is_some_and(|preview| preview.categories.len() > 1), checked: !workbench.refactor_preview_group_by_file(), alt: None },
                ];
                let context = workbench.key_context(window);
                workbench.show_context_menu_actions(actions, &context, gpui::Bounds { origin: event.position, size: gpui::Size::default() }, Vec::new(), window, cx);
            }))
            .into_any_element()
    }

    /// `CategoryElementRenderer`: the category's icon, label and description.
    fn refactor_preview_category(&self, category: &Category, foreground: gpui::Rgba) -> Div {
        let icon = category.icon["id"].as_str().or(category.icon.as_str()).unwrap_or("symbol-file").to_string();
        div()
            .flex()
            .items_center()
            .child(self.icon(&icon, 16., 16., foreground).mr(px(4.)))
            .child(category.label.clone())
            .children(category.description.clone().map(|description| div().ml(px(6.)).opacity(0.7).text_size(px(11.7)).child(description)))
    }

    /// `FileElementRenderer`: file icon and name with its folder; "old → new" with "(renaming)", "(creating)", or
    /// struck through with "(deleting)".
    fn refactor_preview_file(&self, file: &FileOperation, foreground: gpui::Rgba) -> Div {
        let path = PathBuf::from(file.uri["path"].as_str().unwrap_or_default());
        let workspace = self.workspace();
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&path, self.languages.language_of(&path, ""), self.theme.kind, 13., ROW_HEIGHT, foreground));
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = path.parent().map(|folder| crate::quick_input::path_label(folder, workspace)).filter(|label| !label.is_empty());
        let details = |text: &'static str| div().ml(px(6.)).opacity(0.7).text_size(px(11.7)).child(text);
        let row = div().flex().items_center().children(icon.map(|icon| icon.mr(px(6.))));
        if file.kind & RENAME != 0 {
            let new_path = file.new_uri.as_ref().and_then(|uri| uri["path"].as_str()).map(PathBuf::from).unwrap_or_default();
            let label = format!("{} \u{2192} {}", crate::quick_input::path_label(&path, workspace), crate::quick_input::path_label(&new_path, workspace));
            return row.child(label).child(details("(renaming)"));
        }
        let row = row.child(div().when(file.kind & DELETE != 0, |name| name.line_through()).child(name)).children(folder.map(|folder| div().ml(px(6.)).opacity(0.7).text_size(px(11.7)).child(folder)));
        match file.kind {
            kind if kind & CREATE != 0 => row.child(details("(creating)")),
            kind if kind & DELETE != 0 => row.child(details("(deleting)")),
            _ => row,
        }
    }

    /// `TextEditElementRenderer`: the edited line's prefix, the replaced text struck through on
    /// `diffEditor.removedTextBackground`, the new text on `diffEditor.insertedTextBackground`, and the suffix.
    fn refactor_preview_text_edit(&self, file: &FileOperation, edit: usize) -> Div {
        let preview = self.refactor_preview.preview.as_ref().expect("rows draw with a preview");
        let text_edit = &preview.edits[edit]["textEdit"];
        let (scheme, path) = (file.uri["scheme"].as_str().unwrap_or("file"), PathBuf::from(file.uri["path"].as_str().unwrap_or_default()));
        let lines: Vec<String> = match self.editors.model(scheme, &path) {
            Some(model) => model.document().lines.clone(),
            None => std::fs::read_to_string(&path).map(|text| text.split('\n').map(str::to_string).collect()).unwrap_or_default(),
        };
        let (start, end) = edit_range(&preview.edits[edit]);
        let token_starts = |line: usize| -> Vec<usize> {
            let Some(model) = self.editors.model(scheme, &path) else { return vec![0] };
            let Some(text) = lines.get(line.wrapping_sub(1)) else { return vec![0] };
            let mut starts = vec![0];
            for (end, ..) in model.document().tokens.line_tokens(&lines, line - 1) {
                starts.push(text[..end.min(text.len())].chars().count());
            }
            starts.pop();
            starts
        };
        let (prefix, selecting, suffix) = text_edit_parts(&lines, start, end, &token_starts(start.0), &token_starts(end.0));
        let inserting = text_edit["text"].as_str().unwrap_or_default().to_string();
        let span = |text: String| div().child(text.replace('\n', " "));
        div()
            .flex()
            .items_center()
            .child(span(prefix))
            .child(span(selecting).line_through().bg(self.color("diffEditor.removedTextBackground")))
            .child(span(inserting).bg(self.color("diffEditor.insertedTextBackground")))
            .child(span(suffix))
    }

    fn toggle_refactor_preview_row(&mut self, row: &Row) {
        let preview = self.refactor_preview.preview.as_mut().expect("rows draw with a preview");
        match row {
            Row::Category(category) => {
                let value = !preview.category_checked(*category);
                preview.set_category_checked(*category, value);
            }
            Row::File(category, file) => {
                let value = !preview.file_checked(file, category.is_some());
                preview.set_file_checked(file, category.is_some(), value);
            }
            Row::TextEdit(_, file, edit) => {
                let value = !preview.checked[*edit];
                preview.set_text_edit_checked(file, *edit, value);
            }
        }
    }

    /// `_openElementInMultiDiffEditor` for one file: the file beside its preview, the edit's range revealed.
    fn open_refactor_preview_row(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let (file, edit) = match row {
            Row::File(_, file) => (file.clone(), None),
            Row::TextEdit(_, file, edit) => (file.clone(), Some(*edit)),
            Row::Category(_) => return,
        };
        if file.kind != TEXT_EDIT {
            return;
        }
        let preview = self.refactor_preview.preview.as_ref().expect("rows draw with a preview");
        let scheme = file.uri["scheme"].as_str().unwrap_or("file").to_string();
        let path = PathBuf::from(file.uri["path"].as_str().unwrap_or_default());
        let original = match self.editors.model(&scheme, &path) {
            Some(model) => model.document().lines.join("\n"),
            None => std::fs::read_to_string(&path).unwrap_or_default(),
        };
        let edits = file.text_edits.iter().filter(|&&index| preview.checked[index]).map(|&index| {
            let (start, end) = edit_range(&preview.edits[index]);
            (start, end, preview.edits[index]["textEdit"]["text"].as_str().unwrap_or_default().to_string())
        });
        let text = apply_text_edits(&original, edits.collect());
        let selection = edit.map(|index| preview.edits[index]["textEdit"]["range"].clone()).unwrap_or(Value::Null);
        self.set_refactor_preview_model(&path, &text, cx);
        let original = Side { scheme, path: path.clone() };
        let modified = Side { scheme: PREVIEW_SCHEME.into(), path };
        let label = format!("{} (Refactor Preview)", original.name());
        self.show_diff_editor(original, modified, Some(label), &json!({ "preserveFocus": true, "selection": selection }), window, cx);
    }

    /// The preview model of `path` with `text`: created once, then replaced whole on each open.
    fn set_refactor_preview_model(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) {
        match self.editors.model_mut(PREVIEW_SCHEME, path) {
            Some(model) => {
                let end = document_end(&model.document().lines);
                drop(model.execute_edits(vec![(Position::new(0, 0), end, text.to_string(), true)], None, EditReason::ApplyEdits));
            }
            None => {
                let document = TextDocument::from_text(PREVIEW_SCHEME, path, text, "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
                self.editors.open_model(document);
                self.editors_changed(cx);
            }
        }
    }
}

fn row_key(category: Option<usize>, file: &str) -> String {
    match category {
        Some(category) => format!("{category}/{file}"),
        None => file.to_string(),
    }
}

/// A text edit's range as 1-based (line, column) pairs.
fn edit_range(edit: &Value) -> ((usize, usize), (usize, usize)) {
    let range = &edit["textEdit"]["range"];
    let at = |key: &str| range[key].as_u64().unwrap_or(1) as usize;
    ((at("startLineNumber"), at("startColumn")), (at("endLineNumber"), at("endColumn")))
}

fn range_start(edit: &Value) -> (usize, usize) {
    edit_range(edit).0
}
