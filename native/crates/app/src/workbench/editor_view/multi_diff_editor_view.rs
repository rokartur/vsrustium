//! The multi diff editor (multiDiffEditor.ts, multiDiffEditorWidgetImpl.ts, diffEditorItemTemplate.ts): one list of
//! the files that scrolls as a whole, each a sticky header (collapse chevron, file icon, name, folder, status, the
//! `multiDiffEditor/resource/title` toolbar and Open File) over the file's read-only diff, side by side or inline
//! below the 900px breakpoint, a binary file placeholder where a model does not load. Opened by
//! `_workbench.openMultiDiffEditor` (editorCommands.ts) and `_workbench.openScmMultiDiffEditor`
//! (scmMultiDiffSourceResolver.ts).
// shortcut: the diffs are read-only and do not scroll horizontally (long lines clip); until the multi diff editor
// hosts code editors.

use std::collections::HashMap;
use std::path::Path;

use gpui::{AnyElement, BoxShadow, Context, Div, FontWeight, ScrollWheelEvent, StyledText, Window, canvas, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::diff_editor_view::{char_ranges, diagonal_fill, diff_decorations_width};
use super::unchanged_regions_view::{BAR_BOX, PLACEHOLDER_HEIGHT};
use super::{EDITOR_FONT, EditorMetrics, Workbench};
use crate::commands::Outcome;
use crate::contextkey::Expr;
use crate::diff_editor::{Side, ZoneSide, diff_options};
use crate::editor::{LINE_HEIGHT, OpenEditor, TextDocument, expand_tabs, split_query, uri_value};
use crate::menus::{Entry, MenuId};
use crate::multi_diff_editor::{EMPTY_MODEL, FileDiff, HEADER_HEIGHT, MULTI_DIFF_EDITOR_SCHEME, MultiDiffInput, Row, sticky_header_top};
use crate::workbench::hover::{HoverContent, HoverOptions};
use crate::workbench::host_requests::uri_parts;

/// multiDiffEditor.ts `MultiDiffEditor.ID`.
const MULTI_DIFF_EDITOR_ID: &str = "multiDiffEditor";
/// diffEditorItemTemplate.ts `binaryFilePlaceholderContentHeight`.
const BINARY_PLACEHOLDER_HEIGHT: f32 = 100.;
/// common/multiDiffEditor.ts `multiDiffEditorExperimentalFileListSetting`, off by default.
const FILE_LIST_SETTING: &str = "multiDiffEditor.experimental.fileList";
/// common/multiDiffEditor.ts `multiDiffEditorFileListHiddenStorageKey` (StorageScope.PROFILE).
const FILE_LIST_HIDDEN_KEY: &str = "multiDiffEditor.fileListHidden";
/// multiDiffEditorWithFileListWidget.ts `DEFAULT_FILE_LIST_WIDTH`.
const FILE_LIST_WIDTH: f32 = 260.;

/// actions.ts `MenuId.EditorTitle` items of the multi diff editor: command, icon, title, order and `when`.
const TITLE_ACTIONS: [(&str, &str, &str, f64, &str); 5] = [
    ("multiDiffEditor.goToPreviousChange", "arrow-up", "Go to Previous Change", 1., "activeEditor == 'multiDiffEditor'"),
    ("multiDiffEditor.goToNextChange", "arrow-down", "Go to Next Change", 2., "activeEditor == 'multiDiffEditor'"),
    ("multiDiffEditor.collapseAll", "collapse-all", "Collapse All Diffs", 100., "activeEditor == 'multiDiffEditor' && !multiDiffEditorAllCollapsed"),
    ("multiDiffEditor.expandAll", "expand-all", "Expand All Diffs", 100., "activeEditor == 'multiDiffEditor' && multiDiffEditorAllCollapsed"),
    ("multiDiffEditor.toggleFileList", "list-flat", "Toggle Files", 100001., "activeEditor == 'multiDiffEditor' && config.multiDiffEditor.experimental.fileList && multiDiffEditorFileListAvailable"),
];

/// The lines of a multi diff side: the empty model of an added or deleted file's missing side, or its model's.
enum SideLines<'a> {
    Missing,
    Model(std::cell::Ref<'a, TextDocument>),
}

impl std::ops::Deref for SideLines<'_> {
    type Target = [String];

    fn deref(&self) -> &[String] {
        match self {
            SideLines::Missing => &EMPTY_MODEL,
            SideLines::Model(document) => &document.lines,
        }
    }
}
impl Workbench {
    pub(in crate::workbench) fn register_multi_diff_editor_commands(&mut self) {
        self.commands.register("_workbench.openMultiDiffEditor", |workbench, args, window, cx| workbench.open_multi_diff_editor(args, window, cx));
        self.commands.register("_workbench.openScmMultiDiffEditor", |workbench, args, window, cx| workbench.open_scm_multi_diff_editor(args, window, cx));
        for (id, collapsed) in [("multiDiffEditor.collapseAll", true), ("multiDiffEditor.expandAll", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                if let Some(input) = workbench.active_multi_diff_mut() {
                    input.set_all_collapsed(collapsed);
                    cx.notify();
                }
                Outcome::done()
            });
        }
        for (id, next) in [("multiDiffEditor.goToNextChange", true), ("multiDiffEditor.goToPreviousChange", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                if let Some((file, line)) = workbench.active_multi_diff_mut().and_then(|input| input.navigate(next)) {
                    workbench.scroll_multi_diff_to(file, line);
                    cx.notify();
                }
                Outcome::done()
            });
        }
        self.commands.register("multiDiffEditor.goToFile", |workbench, args, window, cx| workbench.go_to_multi_diff_file(args, window, cx));
        // actions.ts `ToggleFileListAction` (`toggleMultiDiffEditorFileList`) and `FocusFileListAction`, which shows a hidden list.
        // shortcut: Focus File List shows the list without moving keyboard focus into it; until the list takes keyboard navigation.
        for (id, toggle) in [("multiDiffEditor.toggleFileList", true), ("multiDiffEditor.focusFileList", false)] {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.editors.active_multi_diff().is_none() || !workbench.multi_diff_file_list_enabled() {
                    return Outcome::done();
                }
                let mut storage = workbench.profile_storage.lock().unwrap();
                let hidden = storage.get(FILE_LIST_HIDDEN_KEY) == Some("true");
                match (hidden, toggle) {
                    (true, _) => storage.delete(FILE_LIST_HIDDEN_KEY),
                    (false, true) => storage.set(FILE_LIST_HIDDEN_KEY, "true".into()),
                    (false, false) => {}
                }
                drop(storage);
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("multiDiffEditor.openLayoutDebug", |workbench, _, _, cx| {
            workbench.open_multi_diff_layout_debug(cx);
            Outcome::done()
        });
    }

    fn multi_diff_file_list_enabled(&self) -> bool {
        self.setting(FILE_LIST_SETTING) == json!(true)
    }

    /// `observableMultiDiffEditorFileListVisible`: the setting is on and the user has not hidden the list.
    fn multi_diff_file_list_visible(&self) -> bool {
        self.multi_diff_file_list_enabled() && self.profile_storage.lock().unwrap().get(FILE_LIST_HIDDEN_KEY) != Some("true")
    }

    /// actions.ts `OpenMultiDiffEditorLayoutDebugAction`: an `inmemory` jsonl model of the layout state, opened pinned.
    // shortcut: one snapshot line, where upstream's model appends a line per layout change; until the layout is observable.
    fn open_multi_diff_layout_debug(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.editors.active_multi_diff() else { return };
        let side_by_side = self.diff_side_by_side();
        let line_height = self.multi_diff_line_height();
        let mut offset = 0.;
        let mut items = Vec::new();
        for file in &input.files {
            let height = self.multi_diff_file_height(file, side_by_side, line_height);
            let uri = |side: &Option<Side>| side.as_ref().map(|side| crate::editor::uri_string(&side.scheme, &side.path));
            let label = file.label_side().map(|side| split_query(&side.scheme, &side.path).0.to_string_lossy().into_owned());
            items.push(json!({ "label": label, "originalUri": uri(&file.original), "modifiedUri": uri(&file.modified), "collapsed": file.collapsed, "verticalState": { "contentHeight": height, "itemViewportOffset": offset - input.scroll_top } }));
            offset += height;
        }
        let state = json!({ "scrollTop": input.scroll_top, "viewHeight": input.view_height.get(), "contentHeight": offset, "items": items });
        let id = self.editors.editors.len() + self.editors.models.len() + 1;
        let path = format!("/multi-diff-editor-layout-{id}.jsonl");
        let document = TextDocument::from_text("inmemory", Path::new(&path), &format!("{state}\n"), "utf8".into(), Some("jsonl"), &self.languages, &self.settings.lock().unwrap());
        self.editors.open(document, false);
        self.editors_changed(cx);
    }

    /// editorCommands.ts `_workbench.openMultiDiffEditor`: `{ multiDiffSourceUri?, title, resources?, reveal? }`.
    fn open_multi_diff_editor(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let options = args.first().cloned().unwrap_or(Value::Null);
        if options["resources"].is_null() && options["multiDiffSourceUri"]["scheme"] == "scm-history-item" {
            return self.open_scm_history_item_multi_diff(options, window, cx);
        }
        let resources: Vec<(Value, Value, Value)> = options["resources"].as_array().into_iter().flatten().map(|resource| (resource["originalUri"].clone(), resource["modifiedUri"].clone(), Value::Null)).collect();
        let reveal = (options["reveal"]["modifiedUri"].clone(), options["reveal"]["range"]["startLineNumber"].as_u64().map(|line| line as usize));
        let input = match self.multi_diff_input(&options["multiDiffSourceUri"], &options["title"], resources) {
            Ok(input) => input,
            Err(error) => return Outcome::Ready(Err(error)),
        };
        self.open_multi_diff_resources(input, reveal, window, cx)
    }

    /// scmMultiDiffSourceResolver.ts `OpenScmGroupAction`: `{ title, repositoryUri, resourceGroupId }`, the group's
    /// resources sorted by path, with the `scmResourceGroup` and `scmProvider` context keys.
    // shortcut: the group's resources when opened, where upstream's resolver follows the group; until a group change re-resolves open editors.
    fn open_scm_multi_diff_editor(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let options = args.first().cloned().unwrap_or(Value::Null);
        let repository = match uri_parts(&options["repositoryUri"]) {
            Ok((_, path)) => path,
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let group_id = options["resourceGroupId"].as_str().unwrap_or_default();
        let Some(provider) = self.scm.model.providers.iter().find(|provider| uri_parts(&provider.root_uri).is_ok_and(|(_, root)| root == repository)) else { return Outcome::done() };
        let Some(group) = provider.groups.iter().find(|group| group.id == group_id) else { return Outcome::done() };
        let mut resources: Vec<&crate::scm::Resource> = group.resources.iter().collect();
        resources.sort_by_key(|resource| uri_parts(&resource.uri).map(|(_, path)| path).unwrap_or_default());
        let resources = resources
            .into_iter()
            .map(|resource| (resource.multi_diff_original.clone().unwrap_or(Value::Null), resource.multi_diff_modified.clone().unwrap_or(Value::Null), resource.uri.clone()))
            .collect();
        let context_keys = vec![("scmResourceGroup".to_string(), json!(group.id)), ("scmProvider".to_string(), json!(provider.id))];
        let source = json!({ "scheme": "scm-multi-diff-source", "path": format!("{}?{group_id}", repository.display()) });
        let mut input = match self.multi_diff_input(&source, &options["title"], resources) {
            Ok(input) => input,
            Err(error) => return Outcome::Ready(Err(error)),
        };
        input.context_keys = context_keys;
        self.open_multi_diff_resources(input, (Value::Null, None), window, cx)
    }

    /// scmMultiDiffSourceResolver.ts `ScmHistoryItemResolver`: a `scm-history-item` source's query names the repository
    /// (`scm<handle>`), the history item and its parent; the item's changes are the files, each opening its modified URI.
    // shortcut: no `goToFileEditorTitle`, Open File shows the modified resource's own name; until editors take a label.
    fn open_scm_history_item_multi_diff(&mut self, options: Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let query: Value = options["multiDiffSourceUri"]["query"].as_str().and_then(|query| serde_json::from_str(query).ok()).unwrap_or(Value::Null);
        let handle = query["repositoryId"].as_str().and_then(|id| id.strip_prefix("scm")).and_then(|handle| handle.parse::<u64>().ok());
        let (Some(handle), Some(item)) = (handle, query["historyItemId"].as_str()) else { return Outcome::Ready(Err(exthost::main_thread::serialized_error("Invalid scm-history-item source"))) };
        let call = self.host.call_cancellable("ExtHostSCM", "$provideHistoryItemChanges", vec![json!(handle), json!(item), query["historyItemParentId"].clone()]);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let changes = call.await.ok().and_then(Result::ok).unwrap_or(Value::Null);
            let resources: Vec<(Value, Value, Value)> = changes.as_array().into_iter().flatten().map(|change| (change["originalUri"].clone(), change["modifiedUri"].clone(), change["modifiedUri"].clone())).collect();
            let reveal = (options["reveal"]["modifiedUri"].clone(), options["reveal"]["range"]["startLineNumber"].as_u64().map(|line| line as usize));
            let result = workbench.update_in(cx, |workbench, window, cx| {
                let input = workbench.multi_diff_input(&options["multiDiffSourceUri"], &options["title"], resources)?;
                drop(workbench.open_multi_diff_resources(input, reveal, window, cx));
                Ok(Value::Null)
            });
            drop(sender.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// The input of a source URI, a title and (original, modified, go to file) URIs, each possibly null.
    fn multi_diff_input(&self, source: &Value, title: &Value, resources: Vec<(Value, Value, Value)>) -> Result<MultiDiffInput, Value> {
        let mut files = Vec::new();
        for (original, modified, go_to_file) in resources {
            files.push(FileDiff::new(side_of(&original)?, side_of(&modified)?, (!go_to_file.is_null()).then_some(go_to_file)));
        }
        Ok(MultiDiffInput::new(side_of(source)?, title.as_str().map(str::to_string), files))
    }

    /// Opens every side's model, a missing one from its content or file system provider, then shows the editor.
    /// `reveal` is a modified URI and the line to reveal in it.
    fn open_multi_diff_resources(&mut self, input: MultiDiffInput, reveal: (Value, Option<usize>), window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let reveal = side_of(&reveal.0).ok().flatten().map(|side| (side, reveal.1));
        let missing: Vec<&Side> = input.files.iter().flat_map(|file| [&file.original, &file.modified]).flatten().filter(|side| self.editors.model(&side.scheme, &side.path).is_none()).collect();
        let contents: Vec<_> = missing
            .into_iter()
            .map(|side| {
                let uri = uri_value(&side.scheme, &side.path);
                let content = (side.scheme != "file").then(|| self.host.call("workbench", "resolveTextContent", vec![uri.clone()]));
                (uri, content)
            })
            .collect();
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let mut resolved = Vec::new();
            for (uri, content) in contents {
                let content = match content {
                    Some(content) => content.await.ok().and_then(Result::ok),
                    None => Some(Value::Null),
                };
                resolved.push((uri, content));
            }
            let result = workbench.update_in(cx, |workbench, _, cx| {
                for (uri, content) in resolved {
                    // A model that does not load leaves its file with the binary file placeholder.
                    if let Some(content) = content {
                        workbench.try_open_document(&uri, &Value::Null, &content, false, cx).ok();
                    }
                }
                workbench.show_multi_diff_editor(input, reveal, cx);
                Ok(Value::Null)
            });
            drop(sender.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled")))));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `openEditor` of a `MultiDiffEditorInput`: an open one of the same source is activated with the new files,
    /// else a new one opens as a preview; `reveal` makes the file of that modified side active, at the line if any
    /// (multiDiffEditorWidgetImpl.ts `reveal`).
    fn show_multi_diff_editor(&mut self, input: MultiDiffInput, reveal: Option<(Side, Option<usize>)>, cx: &mut Context<Self>) {
        let existing = input.source.as_ref().and_then(|source| self.editors.editors.iter().position(|editor| self.editors.multi_diffs.get(&editor.model_id).is_some_and(|open| open.source.as_ref() == Some(source))));
        match existing {
            Some(index) => {
                self.editors.activate(index);
                let id = self.editors.editors[index].model_id;
                let open = self.editors.multi_diffs.get_mut(&id).expect("found above");
                open.label = input.label;
                open.context_keys = input.context_keys;
                open.set_files(input.files);
            }
            None => {
                let id = self.editors.multi_diffs.len() + self.editors.editors.len() + 1;
                let entry = TextDocument::from_text(MULTI_DIFF_EDITOR_SCHEME, Path::new(&format!("/{id}")), "", "utf8".into(), Some("plaintext"), &self.languages, &self.settings.lock().unwrap());
                self.editors.open_multi_diff(entry, input, true);
            }
        }
        self.marked_text = None;
        self.update_multi_diff_editor();
        if let Some((side, line)) = reveal {
            let input = self.active_multi_diff_mut().expect("shown above");
            if let Some(file) = input.files.iter().position(|file| file.modified.as_ref() == Some(&side)) {
                input.reveal(file, line);
                self.scroll_multi_diff_to(file, line);
            }
        }
        self.editors_changed(cx);
    }

    /// Recomputes the diffs of the active multi diff editor's files whose models changed, with
    /// `diffEditor.hideUnchangedRegions` counts; a file with a side whose model is not open is unloadable.
    pub(in crate::workbench) fn update_multi_diff_editor(&mut self) {
        let Some(id) = self.editors.active_tab().map(|tab| tab.model_id).filter(|id| self.editors.multi_diffs.contains_key(id)) else { return };
        let mut input = self.editors.multi_diffs.remove(&id).expect("checked above");
        let options = diff_options(|key| self.setting(key));
        let count = |key: &str| self.setting(key).as_u64().unwrap_or(3) as usize;
        let (min_hidden, min_context) = (count("diffEditor.hideUnchangedRegions.minimumLineCount"), count("diffEditor.hideUnchangedRegions.contextLineCount"));
        for file in &mut input.files {
            let version = |side: &Option<Side>| match side {
                None => Some(0),
                Some(side) => self.editors.model(&side.scheme, &side.path).map(|model| model.text_model().version_id),
            };
            match (self.multi_diff_lines(&file.original).zip(version(&file.original)), self.multi_diff_lines(&file.modified).zip(version(&file.modified))) {
                (Some((original, original_version)), Some((modified, modified_version))) => {
                    file.unloadable = false;
                    file.update((&original, original_version), (&modified, modified_version), options, min_hidden, min_context);
                }
                _ => file.unloadable = true,
            }
        }
        self.editors.multi_diffs.insert(id, input);
    }

    /// The context keys of an active multi diff editor: `activeEditor` and `multiDiffEditorAllCollapsed`.
    pub(in crate::workbench) fn insert_multi_diff_editor_context(&self, context: &mut HashMap<String, Value>) {
        let Some(input) = self.editors.active_multi_diff() else { return };
        context.insert("activeEditor".into(), json!(MULTI_DIFF_EDITOR_ID));
        context.insert("multiDiffEditorAllCollapsed".into(), json!(input.all_collapsed()));
        let enabled = self.multi_diff_file_list_enabled();
        context.insert(format!("config.{FILE_LIST_SETTING}"), json!(enabled));
        context.insert("multiDiffEditorFileListAvailable".into(), json!(enabled));
        context.insert("multiDiffEditorFileListEnabled".into(), json!(self.multi_diff_file_list_visible()));
        context.insert("multiDiffEditorFileListVisible".into(), json!(self.multi_diff_file_list_visible()));
    }

    fn active_multi_diff_mut(&mut self) -> Option<&mut MultiDiffInput> {
        let id = self.editors.active_tab()?.model_id;
        self.editors.multi_diffs.get_mut(&id)
    }

    /// Scrolls the active multi diff editor to file `index`: its top, or `line` (1-based, modified) in the middle.
    fn scroll_multi_diff_to(&mut self, index: usize, line: Option<usize>) {
        let side_by_side = self.diff_side_by_side();
        let line_height = self.multi_diff_line_height();
        let input = self.editors.active_multi_diff().expect("called on the active multi diff editor");
        let heights: Vec<f32> = input.files.iter().map(|file| self.multi_diff_file_height(file, side_by_side, line_height)).collect();
        let file_top: f32 = heights[..index].iter().sum();
        let total: f32 = heights.iter().sum();
        let view_height = input.view_height.get();
        let target = match line {
            None => file_top,
            Some(line) => {
                let rows = self.multi_diff_rows(&input.files[index], side_by_side);
                let row = rows.iter().position(|row| matches!(row, Row::Line { modified: Some(modified), .. } if *modified + 1 == line)).unwrap_or(0);
                let row_top = file_top + HEADER_HEIGHT + rows_height(&rows[..row], line_height);
                row_top - (view_height - line_height) / 2.
            }
        };
        let max = (total - view_height).max(0.);
        self.active_multi_diff_mut().expect("found above").scroll_top = target.clamp(0., max);
    }

    /// actions.ts `GoToFileAction`: opens the file's `goToFileUri`, else the resource the header passed.
    fn go_to_multi_diff_file(&mut self, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(uri) = args.into_iter().next() else { return Outcome::done() };
        let Some(input) = self.editors.active_multi_diff() else { return Outcome::done() };
        let side = uri_parts(&uri).ok().map(|(scheme, path)| Side { scheme: scheme.to_string(), path });
        let file = input.files.iter().find(|file| side.is_some() && (file.modified == side || file.original == side));
        let target = file.and_then(|file| file.go_to_file.clone()).unwrap_or(uri);
        self.execute_command("vscode.open", vec![target], window, cx)
    }

    fn multi_diff_line_height(&self) -> f32 {
        LINE_HEIGHT * self.editor_zoom.multiplier()
    }

    /// A side's lines: an empty model without a resource, none while its model is not open.
    fn multi_diff_lines(&self, side: &Option<Side>) -> Option<SideLines<'_>> {
        match side {
            None => Some(SideLines::Missing),
            Some(side) => self.editors.model(&side.scheme, &side.path).map(|model| SideLines::Model(model.document())),
        }
    }

    fn multi_diff_rows(&self, file: &FileDiff, side_by_side: bool) -> Vec<Row> {
        if file.collapsed {
            return Vec::new();
        }
        let (Some(original), Some(modified)) = (self.multi_diff_lines(&file.original), self.multi_diff_lines(&file.modified)) else { return Vec::new() };
        file.rows(&original, modified.len(), side_by_side)
    }

    /// A file's header and, expanded, its diff's content height or the binary file placeholder's.
    fn multi_diff_file_height(&self, file: &FileDiff, side_by_side: bool, line_height: f32) -> f32 {
        if file.unloadable && !file.collapsed {
            return HEADER_HEIGHT + BINARY_PLACEHOLDER_HEIGHT;
        }
        HEADER_HEIGHT + rows_height(&self.multi_diff_rows(file, side_by_side), line_height)
    }

    /// The editor title's `navigation` actions whose `when` holds in `context`.
    pub(super) fn multi_diff_title_actions(&self, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Vec<(f64, AnyElement)> {
        let mut actions = Vec::new();
        for (command, icon, title, order, when) in TITLE_ACTIONS {
            if Expr::deserialize(when).is_some_and(|when| when.evaluate(context)) {
                actions.push((order, self.multi_diff_action(command, icon, title, Vec::new(), cx).into_any_element()));
            }
        }
        actions
    }

    fn multi_diff_action(&self, command: &'static str, icon: &str, title: &'static str, args: Vec<Value>, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let id = gpui::SharedString::from(format!("multi-diff-{command}-{}", args.first().map(Value::to_string).unwrap_or_default()));
        self.icon(icon, 22., 16., self.color("icon.foreground"))
            .id(id.clone())
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|icon| icon.bg(self.color("toolbar.hoverBackground")))
            .on_click(cx.listener(move |workbench, _, window, cx| {
                cx.stop_propagation();
                drop(workbench.execute_command(command, args.clone(), window, cx));
            }))
            .child(self.hover_target(gpui::SharedString::from(format!("{id}-hover")), HoverContent::Action { title, command }, HoverOptions::mouse()))
    }

    /// multiDiffEditorWithFileListWidget.ts: the diffs, with the file list right of them while it is visible.
    pub(super) fn multi_diff_editor_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let list = self.multi_diff_list(window, cx);
        let body = div().flex_1().min_h_0().flex().child(list);
        match self.multi_diff_file_list_visible() {
            true => body.child(self.multi_diff_file_list(cx)),
            false => body,
        }
    }

    /// multiDiffEditorFileList.ts in its default place right of the diffs: one row per file, its label and status
    /// letter; a click reveals the file.
    // shortcut: a flat list at the default width, without the tree view mode, the left position or a sash; until requested.
    fn multi_diff_file_list(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let input = self.editors.active_multi_diff().expect("drawn for the active multi diff editor");
        let (foreground, description) = (self.color("foreground"), self.color("descriptionForeground"));
        let active = input.cursor.map(|cursor| cursor.0);
        let rows = input.files.iter().enumerate().map(|(index, file)| {
            let (letter, color) = match file.status() {
                Some("A") => ("A", "gitDecoration.addedResourceForeground"),
                Some("D") => ("D", "gitDecoration.deletedResourceForeground"),
                Some("R") => ("R", "gitDecoration.modifiedResourceForeground"),
                _ => ("M", "gitDecoration.modifiedResourceForeground"),
            };
            div()
                .id(("multi-diff-file-list-item", index))
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(active == Some(index), |row| row.bg(self.color("list.inactiveSelectionBackground")))
                .hover(|row| row.bg(self.color("list.hoverBackground")))
                .child(div().flex_1().min_w_0().overflow_hidden().children(file.label_side().map(|side| self.multi_diff_label(side, file.modified.is_none(), foreground, description))))
                .child(div().flex_none().ml(px(8.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(self.color(color)).child(letter))
                .on_click(cx.listener(move |workbench, _, _, cx| {
                    let Some(input) = workbench.active_multi_diff_mut() else { return };
                    if index < input.files.len() {
                        input.reveal(index, None);
                        workbench.scroll_multi_diff_to(index, None);
                        cx.notify();
                    }
                }))
        });
        div()
            .id("multi-diff-file-list")
            .w(px(FILE_LIST_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .text_color(foreground)
            .bg(self.color("multiDiffEditor.background"))
            .children(rows)
    }

    /// The diffs: the files' visible rows and placeholders, the unchanged regions' placeholders over them (their bars
    /// reach into the lines around), then the headers on top, each stuck to the top while its file scrolls under it.
    fn multi_diff_list(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let id = self.editors.active_tab().expect("drawn for the active tab").model_id;
        let input = &self.editors.multi_diffs[&id];
        let (part_width, view_height) = (self.diff_part_width.clone(), input.view_height.clone());
        let measure = canvas(
            move |bounds, _, _| {
                part_width.set(f32::from(bounds.size.width));
                view_height.set(f32::from(bounds.size.height));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let mut toolbar_context = self.key_context(window);
        toolbar_context.extend(input.context_keys.iter().cloned());
        let side_by_side = self.diff_side_by_side();
        let line_height = self.multi_diff_line_height();
        let (scroll_top, height) = (input.scroll_top, input.view_height.get());
        let visible = |top: f32, bottom: f32| bottom >= scroll_top && top <= scroll_top + height;
        let (mut rows, mut placeholders, mut headers) = (Vec::new(), Vec::new(), Vec::new());
        let mut file_top = 0.;
        for (index, file) in input.files.iter().enumerate() {
            let file_rows = self.multi_diff_rows(file, side_by_side);
            let file_height = self.multi_diff_file_height(file, side_by_side, line_height);
            if visible(file_top, file_top + file_height) {
                let metrics = self.multi_diff_metrics(file, window);
                let cursor = input.cursor.filter(|cursor| cursor.0 == index).map(|cursor| cursor.1);
                let mut top = file_top + HEADER_HEIGHT;
                if file.unloadable && !file.collapsed {
                    rows.push(self.multi_diff_binary_placeholder(index, file, cx).absolute().top(px(top - scroll_top)).left_0().right_0().h(px(BINARY_PLACEHOLDER_HEIGHT)));
                }
                for row in file_rows {
                    let row_height = if matches!(row, Row::Hidden { .. }) { PLACEHOLDER_HEIGHT } else { line_height };
                    if visible(top, top + row_height) {
                        let element = self.multi_diff_row(id, index, file, row, side_by_side, cursor, metrics, cx).absolute().top(px(top - scroll_top)).left_0().right_0().h(px(row_height));
                        if matches!(row, Row::Hidden { .. }) { placeholders.push(element) } else { rows.push(element) }
                    }
                    top += row_height;
                }
                let header_top = sticky_header_top(file_top, file_height, scroll_top);
                headers.push(self.multi_diff_header(index, file, header_top > file_top, &toolbar_context, cx).absolute().top(px(header_top - scroll_top)).left_0().right_0());
            }
            file_top += file_height;
        }
        let total_height = file_top;
        div()
            .id("multi-diff-editor")
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(self.color("multiDiffEditor.background"))
            .font_family(EDITOR_FONT)
            .child(measure)
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| {
                let line_height = workbench.multi_diff_line_height();
                let Some(input) = workbench.editors.multi_diffs.get_mut(&id) else { return };
                let delta = f32::from(event.delta.pixel_delta(px(line_height)).y);
                let max = (total_height - input.view_height.get()).max(0.);
                input.scroll_top = (input.scroll_top - delta).clamp(0., max);
                cx.stop_propagation();
                cx.notify();
            }))
            .children(rows)
            .children(placeholders)
            .children(headers)
    }

    /// A file's editor metrics: line numbers as wide as the longer side's last number.
    fn multi_diff_metrics(&self, file: &FileDiff, window: &mut Window) -> EditorMetrics {
        let line_count = [&file.original, &file.modified].into_iter().filter_map(|side| self.multi_diff_lines(side)).map(|lines| lines.len()).max().unwrap_or(1);
        EditorMetrics::measure(window, line_count, super::LineNumbers::On, self.editor_zoom.multiplier())
    }

    /// diffEditorItemTemplate.ts header: the collapse chevron, the file's icon, name and folder, the status flag, the
    /// original name of a renamed file struck through, then the `MultiDiffEditorFileToolbar` (the extensions'
    /// `navigation` items and Open File) run with the file's resource; a shadow while the file scrolls under it.
    fn multi_diff_header(&self, index: usize, file: &FileDiff, stuck: bool, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("foreground");
        let description = self.color("descriptionForeground");
        let chevron = if file.collapsed { "chevron-right" } else { "chevron-down" };
        let id = self.editors.active_tab().expect("drawn for the active tab").model_id;
        let collapse = self.icon(chevron, 16., 16., foreground).id(("multi-diff-collapse", index)).mx(px(6.)).cursor_pointer().on_click(cx.listener(move |workbench, _, _, cx| {
            let Some(input) = workbench.editors.multi_diffs.get_mut(&id) else { return };
            if let Some(file) = input.files.get_mut(index) {
                file.collapsed = !file.collapsed;
                input.cursor = Some((index, 1));
                cx.notify();
            }
        }));
        let label = file.label_side().map(|side| self.multi_diff_label(side, file.modified.is_none(), foreground, description));
        let renamed = file.original.as_ref().filter(|_| file.status() == Some("R")).map(|original| self.multi_diff_label(original, true, foreground, description));
        let status = file.status().map(|status| div().mx(px(10.)).line_height(px(22.)).font_weight(FontWeight::SEMIBOLD).opacity(0.75).child(status));
        let resource = file.label_side().map(|side| uri_value(&side.scheme, &side.path));
        let toolbar = resource.map(|resource| self.multi_diff_toolbar(resource, context, cx)).unwrap_or_default();
        let border = self.theme.color("multiDiffEditor.border");
        let bottom_border = self.theme.color("sideBarSectionHeader.border").filter(|_| !file.collapsed);
        let shadow = self.theme.color("scrollbar.shadow").filter(|_| stuck);
        let content = div()
            .mt(px(8.))
            .px(px(6.))
            .py(px(4.))
            .flex()
            .items_center()
            .text_color(foreground)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .bg(self.color("multiDiffEditor.headerBackground"))
            .when_some(border, |content, border| content.border_t_1().border_color(border))
            .when_some(bottom_border, |content, border| content.border_b_1().border_color(border))
            .when_some(shadow, |content, shadow| content.shadow(vec![BoxShadow { color: shadow.into(), offset: point(px(0.), px(6.)), blur_radius: px(6.), spread_radius: px(-6.), inset: false }]))
            .child(collapse)
            .child(div().flex_1().min_w_0().flex().items_center().overflow_hidden().children(label).children(status).children(renamed))
            .child(div().flex_none().flex().items_center().px(px(8.)).children(toolbar));
        div().h(px(HEADER_HEIGHT)).bg(self.color("editor.background")).child(content)
    }

    /// The `MultiDiffEditorFileToolbar` primary group by order: the `navigation*` items whose `when` holds, with
    /// actions.ts Open File at order 22.
    fn multi_diff_toolbar(&self, resource: Value, context: &HashMap<String, Value>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut items: Vec<(f64, AnyElement)> = vec![(22., self.multi_diff_action("multiDiffEditor.goToFile", "go-to-file", "Open File", vec![resource.clone()], cx).into_any_element())];
        let foreground = self.color("icon.foreground");
        for item in self.menus.iter().filter(|item| item.menu == MenuId::MultiDiffEditorFileToolbar && item.group.starts_with("navigation") && item.when.as_ref().is_none_or(|when| when.evaluate(context))) {
            let Entry::Command { id, .. } = &item.entry else { continue };
            items.push((item.order, self.command_button(id, item.title.clone(), vec![resource.clone()], foreground, cx).into_any_element()));
        }
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        items.into_iter().map(|(_, element)| element).collect()
    }

    /// A `ResourceLabel` of a side: file icon, name (14px) and the folder as its description.
    fn multi_diff_label(&self, side: &Side, strikethrough: bool, foreground: gpui::Rgba, description: gpui::Rgba) -> Div {
        let path = split_query(&side.scheme, &side.path).0;
        let language = self.editors.model(&side.scheme, &side.path).map(|model| model.document().language_id.clone()).unwrap_or_default();
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(path, &language, self.theme.kind, 13., 22., foreground));
        let folder = path.parent().map(|folder| crate::quick_input::path_label(folder, self.workspace())).unwrap_or_default();
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        div()
            .flex_none()
            .flex()
            .items_center()
            .line_height(px(22.))
            .whitespace_nowrap()
            .children(icon.map(|icon| icon.mr(px(6.))))
            .child(div().text_size(px(14.)).when(strikethrough, |name| name.line_through()).child(name))
            .when(!folder.is_empty(), |label| label.child(div().ml(px(6.)).text_size(px(12.)).text_color(description).opacity(0.7).child(folder)))
    }

    /// diffEditorItemTemplate.ts `binary-file-placeholder`: "Binary file changed" and, with both sides, Open Diff,
    /// which opens the text diff editor pinned (`openDiffEditor`).
    fn multi_diff_binary_placeholder(&self, index: usize, file: &FileDiff, cx: &mut Context<Self>) -> Div {
        let sides = file.original.as_ref().zip(file.modified.as_ref()).map(|(original, modified)| (uri_value(&original.scheme, &original.path), uri_value(&modified.scheme, &modified.path)));
        let open_diff = sides.map(|(original, modified)| {
            div()
                .id(("multi-diff-open-binary", index))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .bg(self.color("button.secondaryBackground"))
                .text_color(self.color("button.secondaryForeground"))
                .hover(|button| button.bg(self.color("button.secondaryHoverBackground")))
                .child("Open Diff")
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    drop(workbench.execute_command("_workbench.diff", vec![original.clone(), modified.clone(), Value::Null, json!([null, { "pinned": true }])], window, cx));
                }))
        });
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .bg(self.color("editor.background"))
            .text_color(self.color("descriptionForeground"))
            .child("Binary file changed")
            .children(open_diff)
    }

    /// One row of a file: side by side the original cell (or its fill) and the modified one; inline one cell with
    /// both line numbers; or a hidden region's placeholder. `cursor` is the file's cursor line, if it is active.
    #[allow(clippy::too_many_arguments, reason = "the row's place, its file and the shared metrics")]
    fn multi_diff_row(&self, id: u32, index: usize, file: &FileDiff, row: Row, side_by_side: bool, cursor: Option<usize>, metrics: EditorMetrics, cx: &mut Context<Self>) -> Div {
        let model = |side: &Option<Side>| side.as_ref().and_then(|side| self.editors.model(&side.scheme, &side.path));
        let (original, modified) = (model(&file.original), model(&file.modified));
        let is_cursor = |line: usize| cursor == Some(line + 1);
        match row {
            Row::Hidden { region, .. } => self.multi_diff_hidden(id, index, file, region, metrics, cx),
            Row::Line { original: original_line, modified: modified_line } if side_by_side => {
                let width = (self.diff_part_width.get() / 2.).floor();
                let border = self.theme.color("diffEditor.border");
                let cell = |side: ZoneSide, model: Option<&OpenEditor>, line: Option<usize>, highlighted: bool| match line {
                    Some(line) => self.multi_diff_line(file, side, model, line, &[Some(line)], highlighted, metrics),
                    None => self.multi_diff_fill(metrics),
                };
                let highlighted = modified_line.is_some_and(is_cursor);
                div()
                    .flex()
                    .child(cell(ZoneSide::Original, original, original_line, false).w(px(width)).flex_none())
                    .child(cell(ZoneSide::Modified, modified, modified_line, highlighted).flex_1().min_w_0().when_some(border, |cell, border| cell.border_l_1().border_color(border)))
            }
            Row::Line { original: original_line, modified: Some(line) } => self.multi_diff_line(file, ZoneSide::Modified, modified, line, &[original_line, Some(line)], is_cursor(line), metrics),
            Row::Line { original: original_line, modified: None } => {
                let line = original_line.expect("a row shows at least one line");
                self.multi_diff_line(file, ZoneSide::Original, original, line, &[Some(line), None], false, metrics)
            }
        }
    }

    /// One read-only line of `side` (0-based `index`): margin background and sign, the `numbers` lanes, the cursor
    /// line's highlight, unchanged code, line and character backgrounds of its change, indent guides and the
    /// highlighted text, as diff_editor_view.rs `original_line` draws the original side.
    #[allow(clippy::too_many_arguments, reason = "the line, its lanes and the shared metrics")]
    fn multi_diff_line(&self, file: &FileDiff, side: ZoneSide, model: Option<&OpenEditor>, index: usize, numbers: &[Option<usize>], highlighted: bool, metrics: EditorMetrics) -> Div {
        let line_number = index + 1;
        let changes = file.diff.as_ref().map_or(&[][..], |diff| &diff.diff.changes[..]);
        let change = changes.iter().find(|change| match side {
            ZoneSide::Original => change.original.contains(line_number),
            ZoneSide::Modified => change.modified.contains(line_number),
        });
        let decorations_width = diff_decorations_width(metrics.char_width);
        let content_left = metrics.glyph_margin_width() + numbers.len() as f32 * metrics.line_numbers_width + decorations_width;
        let mut content = div().absolute().top_0().bottom_0().right_0().left(px(content_left));
        if highlighted {
            content = content.child(match self.theme.color("editor.lineHighlightBackground") {
                Some(background) => div().absolute().inset_0().bg(background),
                None => div().absolute().inset_0().border_2().border_color(self.color("editor.lineHighlightBorder")),
            });
        }
        if file.in_unchanged(side, line_number)
            && let Some(background) = self.theme.color("diffEditor.unchangedCodeBackground")
        {
            content = content.child(div().absolute().inset_0().bg(background));
        }
        let document = model.map(|model| model.document());
        let text = document.as_ref().and_then(|document| document.lines.get(index));
        if let Some(change) = change {
            content = content.children(self.line_background(side));
            let other_side_empty = match side {
                ZoneSide::Original => change.modified.is_empty(),
                ZoneSide::Modified => change.original.is_empty(),
            };
            if let (Some(model), Some(text), false) = (model, text, other_side_empty) {
                let ranges = char_ranges(change, side, line_number, text, model.document().indentation.tab_size, metrics.char_width);
                content = content.children(ranges.into_iter().map(|(left, width)| self.char_background(side, left, width)));
            }
        }
        if let (Some(model), Some(text)) = (model, text) {
            let tab_size = model.document().indentation.tab_size;
            for level in 0..model.text_model().guides.get(index).copied().unwrap_or(0) {
                let x = (level * model.document().indentation.indent_size()) as f32 * metrics.char_width;
                content = content.child(div().absolute().top_0().left(px(x)).w(px(1.)).h_full().bg(self.color("editorIndentGuide.background1")));
            }
            let highlights = model.document().tokens.highlights(&self.theme, &self.languages, &model.document().lines, index, tab_size);
            content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
        }
        let number_color = self.color("editorLineNumber.foreground");
        div()
            .relative()
            .h(px(metrics.line_height))
            .flex()
            .overflow_hidden()
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            .text_color(self.color("editor.foreground"))
            .bg(self.color("editor.background"))
            .when(change.is_some(), |row| row.children(self.margin_background(side, content_left)))
            .child(div().w(px(metrics.glyph_margin_width())).flex_none())
            .children(numbers.iter().map(|number| div().w(px(metrics.line_numbers_width)).flex_none().text_right().text_color(number_color).children(number.map(|number| (number + 1).to_string()))))
            .child(div().w(px(decorations_width)).flex_none().when(change.is_some() && self.diff_render_indicators(), |lane| lane.child(self.diff_sign(side, metrics))))
            .child(content)
    }

    /// `createFakeLinesDiv`: the diagonal fill of the side without a line.
    fn multi_diff_fill(&self, metrics: EditorMetrics) -> Div {
        let content_left = metrics.glyph_margin_width() + metrics.line_numbers_width + diff_decorations_width(metrics.char_width);
        let color = self.theme.color("diffEditor.diagonalFill");
        div().relative().h(px(metrics.line_height)).bg(self.color("editor.background")).children(color.map(|color| diagonal_fill(color).absolute().top_0().bottom_0().right_0().left(px(content_left))))
    }

    /// hideUnchangedRegionsFeature.ts `diff-hidden-lines`, as unchanged_regions_view.rs `unchanged_placeholder` draws
    /// it: the unfold icon and "{n} hidden lines" (double click) show the whole region, the bars above and below
    /// show `revealLineCount` more lines on their side.
    // shortcut: the bars click but do not drag, and no outline breadcrumbs; until requested.
    fn multi_diff_hidden(&self, id: u32, index: usize, file: &FileDiff, region: usize, metrics: EditorMetrics, cx: &mut Context<Self>) -> Div {
        let content_left = metrics.glyph_margin_width() + metrics.line_numbers_width + diff_decorations_width(metrics.char_width);
        let hidden_count = file.unchanged[region].hidden_modified().len();
        let element_id = |part: &str| gpui::SharedString::from(format!("multi-diff-hidden-{index}-{region}-{part}"));
        let change = move |workbench: &mut Workbench, cx: &mut Context<Workbench>, change: &dyn Fn(&mut crate::diff_editor::unchanged_regions::UnchangedRegion, usize)| {
            let reveal = workbench.reveal_line_count();
            if let Some(region) = workbench.editors.multi_diffs.get_mut(&id).and_then(|input| input.files.get_mut(index)).and_then(|file| file.unchanged.get_mut(region)) {
                change(region, reveal);
                cx.notify();
            }
        };
        let focus = self.color("focusBorder");
        let bar = |top: bool| {
            let title = if top { "Click or drag to show more above" } else { "Click or drag to show more below" };
            let group = format!("multi-diff-bar-{index}-{region}-{top}");
            div()
                .id(element_id(if top { "top" } else { "bottom" }))
                .group(group.clone())
                .absolute()
                .left_0()
                .right_0()
                .top(px(if top { -6. } else { PLACEHOLDER_HEIGHT - 6. }))
                .h(px(BAR_BOX))
                .cursor_pointer()
                .child(div().absolute().left_0().right_0().top(px(4.)).h(px(4.)).group_hover(group, |fill| fill.bg(focus)))
                .child(self.hover_target(element_id(if top { "top-hover" } else { "bottom-hover" }), HoverContent::Text(title.into()), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, _, _, cx| {
                    change(workbench, cx, &|region, reveal| if top { region.show_more_above(reveal) } else { region.show_more_below(reveal) });
                }))
        };
        let unfold = div()
            .id(element_id("unfold"))
            .cursor_pointer()
            .child(self.icon("unfold", 16., 16., self.color("diffEditor.unchangedRegionForeground")))
            .child(self.hover_target(element_id("unfold-hover"), HoverContent::Text("Show Unchanged Region".into()), HoverOptions::mouse()))
            .on_click(cx.listener(move |workbench, _, _, cx| change(workbench, cx, &|region, _| region.show_all())));
        let label = div()
            .id(element_id("label"))
            .cursor_pointer()
            .child(format!("{hidden_count} hidden lines"))
            .child(self.hover_target(element_id("label-hover"), HoverContent::Text("Double click to unfold".into()), HoverOptions::mouse()))
            .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    change(workbench, cx, &|region, _| region.show_all());
                }
            }));
        let shadow = self.color("diffEditor.unchangedRegionShadow");
        let line = || div().absolute().left_0().right_0().h(px(1.)).bg(gpui::Rgba { a: shadow.a * 0.3, ..shadow });
        let center = div()
            .absolute()
            .inset_0()
            .flex()
            .overflow_hidden()
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .line_height(px(14.))
            .bg(self.color("diffEditor.unchangedRegionBackground"))
            .text_color(self.color("diffEditor.unchangedRegionForeground"))
            .child(line().top_0())
            .child(line().bottom_0())
            .child(div().w(px(content_left)).flex_none().flex().items_center().justify_center().child(unfold))
            .child(div().flex_1().min_w_0().flex().justify_center().items_center().whitespace_nowrap().child(label));
        div().relative().child(center).child(bar(true)).child(bar(false))
    }
}

/// A side's URI, none for null.
fn side_of(uri: &Value) -> Result<Option<Side>, Value> {
    if uri.is_null() {
        return Ok(None);
    }
    let (scheme, path) = uri_parts(uri)?;
    Ok(Some(Side { scheme: scheme.to_string(), path }))
}

fn rows_height(rows: &[Row], line_height: f32) -> f32 {
    rows.iter().map(|row| if matches!(row, Row::Hidden { .. }) { PLACEHOLDER_HEIGHT } else { line_height }).sum()
}

#[cfg(test)]
mod tests;
