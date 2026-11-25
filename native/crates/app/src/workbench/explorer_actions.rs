//! The Explorer's file actions under their upstream command ids (fileActions.ts, fileActions.contribution.ts): new
//! file and folder, rename, delete, copy, cut and paste, the inline name editor of explorerViewer.ts
//! `renderInputBox` with `InlineEditInputBox`, and the undo and redo of the file edits they apply
//! (explorerService.ts `applyBulkEdit`, files.contribution.ts `UndoCommand.addImplementation(110, 'explorer')`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use config::configuration::Overrides;
use files::Resource;
use files::operations::{FileEdit, FileEditOptions, FileUndo};
use gpui::{AsyncWindowContext, Context, Div, DragMoveEvent, ExternalPaths, FocusHandle, FontWeight, HighlightStyle, MouseButton, MouseDownEvent, PromptLevel, Rgba, ScrollHandle, StyledText, Subscription, Task, WeakEntity, Window, deferred, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::explorer_find::ExplorerFind;
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::dialog;
use crate::explorer::{Navigation, NewItem};
use crate::file_actions::{self, Deleted, IncrementalNaming, Severity};
use crate::host_sync::FileJob;
use crate::input_box::InputBox;
use crate::menus::MenuId;
use crate::undo_redo::{Direction, UndoRedo};

#[cfg(test)]
mod tests;

/// `UNDO_REDO_SOURCE` of explorerService.ts.
const UNDO_SOURCE: u64 = 1;
/// `MAX_UNDO_FILE_SIZE` of fileActions.ts: 5 MB.
const MAX_UNDO_FILE_SIZE: f64 = 5_000_000.;

pub struct ExplorerState {
    /// The tree's own keyboard focus; `filesExplorerFocus` also holds while the inline editor has it.
    pub focus: FocusHandle,
    pub input: InputView,
    pub input_focus: FocusHandle,
    /// `ExplorerService.editable`.
    pub edit: Option<ExplorerEdit>,
    /// The folder a drag over the tree would drop into; it and its rows draw `list.dropBackground`.
    pub drop_target: Option<PathBuf>,
    expand_timer: Option<Task<()>>,
    undo: UndoRedo<FileUndo>,
    pub scroll: ScrollHandle,
    pub find: ExplorerFind,
}

pub struct ExplorerEdit {
    /// The renamed item; `None` edits the new item's name.
    pub renamed: Option<PathBuf>,
    pub input: InputBox,
    /// The F2 cycle of the selection: name, everything, extension.
    f2_selection: usize,
    /// Moving the focus elsewhere accepts the value.
    _blur: Subscription,
}

impl ExplorerState {
    pub fn new(cx: &mut gpui::App) -> Self {
        ExplorerState { focus: cx.focus_handle(), input: InputView::default(), input_focus: cx.focus_handle(), edit: None, drop_target: None, expand_timer: None, undo: UndoRedo::default(), scroll: ScrollHandle::new(), find: ExplorerFind::new(cx) }
    }
}

/// `ExplorerView.TREE_VIEW_STATE_STORAGE_KEY`, workspace storage.
const TREE_VIEW_STATE_KEY: &str = "workbench.explorer.treeViewState";

impl Workbench {
    /// `ExplorerView.storeTreeViewState`. Upstream stores on `onWillSaveState`; the port's caller runs it when the
    /// window closes or the app quits.
    pub(super) fn store_explorer_view_state(&self) {
        let Some(tree) = &self.folder else { return };
        let state = tree.view_state(-f32::from(self.explorer.scroll.offset().y));
        self.workspace_storage.lock().unwrap().set(TREE_VIEW_STATE_KEY, state.to_string());
    }

    pub(super) fn restore_explorer_view_state(&mut self) {
        let Some(tree) = &mut self.folder else { return };
        let stored = self.workspace_storage.lock().unwrap().get(TREE_VIEW_STATE_KEY).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        let Some(state) = stored else { return };
        let scroll_top = tree.restore_view_state(&state);
        self.explorer.scroll.set_offset(gpui::point(px(0.), px(-scroll_top)));
    }

    pub(super) fn register_explorer_actions(&mut self) {
        self.commands.register("explorer.newFile", |workbench, _, window, cx| workbench.new_explorer_item(false, window, cx));
        self.commands.register("explorer.newFolder", |workbench, _, window, cx| workbench.new_explorer_item(true, window, cx));
        self.commands.register("renameFile", |workbench, _, window, cx| workbench.rename_explorer_item(window, cx));
        self.commands.register("moveFileToTrash", |workbench, _, window, cx| workbench.delete_explorer_items(true, window, cx));
        self.commands.register("deleteFile", |workbench, _, window, cx| workbench.delete_explorer_items(false, window, cx));
        self.commands.register("filesExplorer.copy", |workbench, _, _, cx| workbench.copy_explorer_items(false, cx));
        self.commands.register("filesExplorer.cut", |workbench, _, _, cx| workbench.copy_explorer_items(true, cx));
        self.commands.register("filesExplorer.cancelCut", |workbench, _, _, cx| {
            if let Some(tree) = workbench.folder.as_mut() {
                tree.clipboard.clear();
                tree.cut = true;
            }
            cx.notify();
            Outcome::done()
        });
        self.commands.register("filesExplorer.paste", |workbench, _, window, cx| workbench.paste_explorer_items(window, cx));
        self.commands.register("copyFilePath", |workbench, args, window, cx| workbench.copy_paths_to_clipboard(false, &args, window, cx));
        self.commands.register("copyRelativeFilePath", |workbench, args, window, cx| workbench.copy_paths_to_clipboard(true, &args, window, cx));
        self.commands.register("revealFileInOS", |workbench, args, window, cx| workbench.reveal_in_os(&args, window, cx));
        self.commands.register("previousCompressedFolder", |workbench, _, _, cx| workbench.move_compressed_focus(Navigation::previous, cx));
        self.commands.register("nextCompressedFolder", |workbench, _, _, cx| workbench.move_compressed_focus(Navigation::next, cx));
        self.commands.register("firstCompressedFolder", |workbench, _, _, cx| workbench.move_compressed_focus(Navigation::first, cx));
        self.commands.register("lastCompressedFolder", |workbench, _, _, cx| workbench.move_compressed_focus(Navigation::last, cx));
    }

    /// explorerView.ts `previousCompressedStat` and its siblings: moves the focused compact row's current folder.
    fn move_compressed_focus(&mut self, step: fn(&mut Navigation), cx: &mut Context<Self>) -> Outcome {
        let Some(tree) = self.folder.as_mut() else { return Outcome::done() };
        let Some(mut navigation) = tree.focused.as_ref().and_then(|focused| tree.navigation_at(focused)) else { return Outcome::done() };
        step(&mut navigation);
        tree.set_navigation(&navigation);
        cx.notify();
        Outcome::done()
    }

    /// files.ts `getMultiSelectedResources`: the Explorer's context while it has focus, else the command's
    /// resource, else the active editor's.
    fn multi_selected_resources(&self, args: &[Value], window: &Window) -> Vec<PathBuf> {
        if let Some(tree) = self.folder.as_ref().filter(|_| self.explorer.focus.is_focused(window)) {
            let context = tree.context(true, true);
            if !context.is_empty() {
                return context;
            }
        }
        if let Some(path) = args.first().and_then(|uri| uri["path"].as_str()) {
            return vec![PathBuf::from(path)];
        }
        self.editors.active().map(|editor| editor.document.path.clone()).into_iter().collect()
    }

    /// fileCommands.ts `resourcesToClipboard`: one path per line, with `explorer.copyPathSeparator` or
    /// `explorer.copyRelativePathSeparator` when it is `/` or `\`.
    fn copy_paths_to_clipboard(&mut self, relative: bool, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let resources = self.multi_selected_resources(args, window);
        if resources.is_empty() {
            return Outcome::done();
        }
        let key = if relative { "explorer.copyRelativePathSeparator" } else { "explorer.copyPathSeparator" };
        let separator = match self.setting(key).as_str() {
            Some(separator @ ("/" | "\\")) => separator.to_string(),
            _ => "/".to_string(),
        };
        let workspace = self.workspace();
        let labels: Vec<String> = resources.iter().map(|path| crate::quick_input::path_label_without_prefix(path, workspace, relative, &separator)).collect();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(labels.join("\n")));
        Outcome::done()
    }

    /// electron-browser fileCommands.ts `revealResourcesInOS`: each resource selected in Finder, else the first
    /// workspace folder.
    fn reveal_in_os(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let mut resources = self.multi_selected_resources(args, window);
        if resources.is_empty() {
            resources.extend(self.workspace().map(|workspace| workspace.folders[0].path.clone()));
        }
        for path in resources {
            cx.reveal_path(&path);
        }
        Outcome::done()
    }

    /// The Explorer's context keys (explorerView.ts `setContextKeys`, `onFocusChanged`, `itemsCopied`).
    pub(super) fn explorer_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let Some(tree) = self.folder.as_ref() else { return };
        // The find widget is inside the tree's element, so the tree keeps the list's focus keys.
        let input_focused = self.explorer.input_focus.is_focused(window) || self.explorer.find.focus.is_focused(window);
        let focused = input_focused || self.explorer.focus.is_focused(window);
        context.insert("foldersViewVisible".into(), json!(true));
        context.insert("filesExplorerFocus".into(), json!(focused));
        context.insert("explorerViewletFocus".into(), json!(focused));
        // The Explorer is the only view of the side bar.
        context.insert("sideBarFocus".into(), json!(focused));
        context.insert("listFocus".into(), json!(focused));
        if input_focused {
            context.insert("inputFocus".into(), json!(true));
        }
        // Without a focused row the keys describe the last workspace folder, a root.
        let stat = tree.focused.clone().unwrap_or_else(|| tree.workspace.folders.last().expect("a workspace has a folder").path.clone());
        let is_root = tree.focused.is_none() || tree.is_root(&stat);
        context.insert("explorerResourceIsRoot".into(), json!(is_root));
        context.insert("explorerResourceIsFolder".into(), json!(is_root || stat.is_dir()));
        // shortcut: no file is read-only; `files.readonlyInclude` and read-only file systems are not ported.
        context.insert("explorerResourceReadonly".into(), json!(false));
        context.insert("explorerResourceParentReadOnly".into(), json!(false));
        context.insert("explorerResourceMoveableToTrash".into(), json!(tree.focused.is_some() && self.setting("files.enableTrash") == json!(true)));
        context.insert("explorerResourceCut".into(), json!(tree.cut && !tree.clipboard.is_empty()));
        context.insert("fileCopied".into(), json!(tree.file_copied));
        let navigation = tree.focused.as_ref().and_then(|focused| tree.navigation_at(focused));
        context.insert("explorerViewletCompressedFocus".into(), json!(navigation.is_some()));
        self.explorer_find_context(context);
        if let Some(navigation) = navigation {
            context.insert("explorerViewletCompressedFirstFocus".into(), json!(navigation.index == 0));
            context.insert("explorerViewletCompressedLastFocus".into(), json!(navigation.index == navigation.items.len() - 1));
        }
        if focused {
            self.explorer_resource_context(tree.focused.as_deref(), context);
        }
    }

    /// explorerView.ts `resourceContext` (contextkeys.ts `ResourceContextKey`) of the focused row, in the view's scope:
    /// while the Explorer has focus it replaces the editor's resource keys, and no focused row clears them.
    fn explorer_resource_context(&self, stat: Option<&Path>, context: &mut HashMap<String, Value>) {
        let keys = ["resource", "resourceScheme", "resourceFilename", "resourceDirname", "resourcePath", "resourceLangId", "resourceExtname", "hasResource", "isFileSystemResource"];
        for key in keys {
            context.remove(key);
        }
        let Some(stat) = stat else { return };
        context.insert("resource".into(), json!(crate::editor::uri_string("file", stat)));
        context.insert("resourceScheme".into(), json!("file"));
        context.insert("resourceFilename".into(), json!(stat.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()));
        context.insert("resourceDirname".into(), json!(stat.parent().map(|parent| parent.to_string_lossy().into_owned()).unwrap_or_default()));
        context.insert("resourcePath".into(), json!(stat.to_string_lossy()));
        context.insert("resourceLangId".into(), json!(self.languages.guess_language(stat, "")));
        context.insert("resourceExtname".into(), json!(stat.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default()));
        context.insert("hasResource".into(), json!(true));
        context.insert("isFileSystemResource".into(), json!(true));
    }

    /// explorerView.ts `onContextMenu`: the ExplorerContext menu for the right-clicked row, or for the space
    /// below the rows. Its commands get the row (else the sole folder) and the selection holding the row.
    pub(super) fn show_explorer_context_menu(&mut self, stat: Option<&Path>, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let tree = self.folder.as_mut().expect("the Explorer shows with a folder");
        tree.context_menu_select(stat);
        tree.file_copied = crate::clipboard::has_resources();
        // `$mid` 1 makes the extension host revive a `vscode.Uri`; markdown.showPreview ignores plain objects.
        let uri = |path: &Path| json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" });
        let (arg, selection) = match stat {
            Some(stat) => {
                // A compact row's menu acts on its current folder.
                let current = tree.navigation_at(stat).map(|navigation| navigation.current().clone());
                (uri(current.as_deref().unwrap_or(stat)), tree.selected.iter().map(|path| uri(path)).collect())
            }
            None if tree.workspace.folders.len() == 1 => (uri(&tree.workspace.folders[0].path), Vec::new()),
            None => (json!({}), Vec::new()),
        };
        window.focus(&self.explorer.focus.clone(), cx);
        let anchor = gpui::Bounds { origin: position, size: gpui::Size::default() };
        let context = self.key_context(window);
        self.show_context_menu(MenuId::ExplorerContext, &context, anchor, vec![arg, Value::Array(selection)], window, cx);
    }

    /// `configurationService.updateValue(key, value)`: written where the key is set, else in user settings.
    pub(super) fn update_setting(&mut self, key: &str, value: Value) {
        let mut settings = self.settings.lock().unwrap();
        match settings.update_value_derived(key, Some(&value)) {
            Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
            Ok(_) => {}
            Err(error) => eprintln!("writing {key}: {error}"),
        }
    }

    pub(super) fn setting(&self, key: &str) -> Value {
        self.settings.lock().unwrap().get_value(Some(key), &Overrides::default()).unwrap_or(Value::Null)
    }

    /// `workbench.tree.indent`, clamped to 0..40 as abstractTree.ts `updateOptions` does; 8 without a number.
    pub(super) fn tree_indent(&self) -> f32 {
        tree_indent(&self.setting("workbench.tree.indent"))
    }

    /// `openExplorerAndCreate`: the new item's row with the name editor, in the focused folder or the focused
    /// file's folder.
    fn new_explorer_item(&mut self, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(tree) = self.folder.as_mut() else {
            if is_dir {
                return Outcome::error("Open a folder or workspace first.");
            }
            return self.execute_command("workbench.action.files.newUntitledFile", Vec::new(), window, cx);
        };
        let parent = match tree.context(false, false).first() {
            Some(stat) => tree.folder_for(stat),
            None => tree.workspace.folders[0].path.clone(),
        };
        tree.expand(&parent);
        tree.new_item = Some(NewItem { parent, is_dir });
        self.start_explorer_edit(None, InputBox::default(), window, cx);
        Outcome::done()
    }

    /// `renameHandler`: the name editor on the focused item, the name before the extension selected.
    fn rename_explorer_item(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(stat) = self.folder.as_ref().and_then(|tree| tree.context(false, false).into_iter().next()) else { return Outcome::done() };
        let name = file_name(&stat);
        let end = match name.rfind('.') {
            Some(dot) if dot > 0 && !stat.is_dir() => name[..dot].chars().count(),
            _ => name.chars().count(),
        };
        let mut input = InputBox::new(name);
        input.select(0, end);
        self.start_explorer_edit(Some(stat), input, window, cx);
        Outcome::done()
    }

    fn start_explorer_edit(&mut self, renamed: Option<PathBuf>, input: InputBox, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.explorer.input_focus.clone();
        let blur = cx.on_blur(&focus, window, |workbench, window, cx| {
            // `InlineEditInputBox`: the window losing focus keeps the edit while the input still has the focus.
            if workbench.explorer.input_focus.is_focused(window) {
                return;
            }
            let valid = workbench.explorer_edit_message().is_none_or(|(_, severity)| severity != Severity::Error);
            workbench.finish_explorer_edit(valid, window, cx);
        });
        self.explorer.input.reset();
        self.explorer.edit = Some(ExplorerEdit { renamed, input, f2_selection: 0, _blur: blur });
        window.focus(&focus, cx);
        self.blink_caret(InputId::Explorer, window, cx);
        cx.notify();
    }

    /// The edited item's folder, and the name it has now (none for the new item).
    fn explorer_edit_parent(&self) -> Option<(PathBuf, Option<String>)> {
        let edit = self.explorer.edit.as_ref()?;
        match &edit.renamed {
            Some(path) => Some((path.parent()?.to_path_buf(), Some(file_name(path)))),
            None => Some((self.folder.as_ref()?.new_item.as_ref()?.parent.clone(), None)),
        }
    }

    /// `validateFileName` of the editor's value.
    pub(super) fn explorer_edit_message(&self) -> Option<(String, Severity)> {
        let (parent, name) = self.explorer_edit_parent()?;
        file_actions::validate_file_name(&parent, name.as_deref(), &self.explorer.edit.as_ref()?.input.value)
    }

    /// explorerviewlet.css `.explorer-item .monaco-inputbox`: a 20px input inside a 1px focusBorder outline
    /// (the message's border while it has one),
    /// and below it the validation message (inputBox.css `.monaco-inputbox-message`).
    pub(super) fn explorer_input_box(&self, label_line: f32, focused: bool, cx: &mut Context<Self>) -> Div {
        let message = self.explorer_edit_message();
        let border = match message.as_ref().map(|(_, severity)| *severity) {
            Some(Severity::Error) => "inputValidation.errorBorder",
            Some(Severity::Warning) => "inputValidation.warningBorder",
            Some(_) => "inputValidation.infoBorder",
            None => "focusBorder",
        };
        let message = message.map(|(message, severity)| {
            let (background, foreground, border) = match severity {
                Severity::Error => ("inputValidation.errorBackground", "inputValidation.errorForeground", "inputValidation.errorBorder"),
                Severity::Warning => ("inputValidation.warningBackground", "inputValidation.warningForeground", "inputValidation.warningBorder"),
                _ => ("inputValidation.infoBackground", "inputValidation.infoForeground", "inputValidation.infoBorder"),
            };
            // Deferred, so it paints over the rows below the edited one as the reference's context view does.
            deferred(
                div()
                    .absolute()
                    .top(px(21.))
                    .left_0()
                    .right_0()
                    .p(px(4.8))
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .bg(self.color(background))
                    .text_color(self.foreground_or(foreground))
                    .border_1()
                    .border_color(self.color(border))
                    .child(markdown_bold(&message)),
            )
        });
        div()
            .relative()
            .h(px(label_line))
            .flex()
            .items_center()
            .child(
                div()
                    .w_full()
                    .h(px(20.))
                    .border_1()
                    .rounded(px(4.))
                    .border_color(self.color(border))
                    .bg(self.color("input.background"))
                    .text_color(self.foreground_or("input.foreground"))
                    .flex()
                    .child(div().flex_1().h_full().child(self.input_text(InputId::Explorer, InputLook { password: false, placeholder: None, focused }, cx))),
            )
            .children(message)
    }

    pub(super) fn edit_explorer_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        if let Some(explorer_edit) = self.explorer.edit.as_mut() {
            edit(&mut explorer_edit.input);
        }
        cx.notify();
    }

    /// Enter accepts a value without an error, Escape cancels, F2 cycles the selection of a file name.
    pub(super) fn explorer_input_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match key {
            "enter" => {
                if self.explorer_edit_message().is_none_or(|(_, severity)| severity != Severity::Error) {
                    self.finish_explorer_edit(true, window, cx);
                }
            }
            "escape" => self.finish_explorer_edit(false, window, cx),
            "f2" => {
                let Some(edit) = self.explorer.edit.as_mut() else { return true };
                let is_dir = edit.renamed.as_ref().is_some_and(|path| path.is_dir());
                let Some(dot) = edit.input.value.rfind('.').filter(|_| !is_dir) else { return true };
                let dot = edit.input.value[..dot].chars().count();
                let len = edit.input.len();
                edit.f2_selection = (edit.f2_selection + 1) % 3;
                match edit.f2_selection {
                    1 => edit.input.select(0, len),
                    2 => edit.input.select(dot + 1, len),
                    _ => edit.input.select(0, dot),
                }
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    /// `onFinish` of the editable data: the editor goes, the tree takes the focus back, then the item is
    /// created or renamed.
    fn finish_explorer_edit(&mut self, success: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.explorer_edit_parent() else { return };
        let Some(edit) = self.explorer.edit.take() else { return };
        let new_item = self.folder.as_mut().and_then(|tree| tree.new_item.take());
        self.explorer.input.reset();
        window.focus(&self.explorer.focus.clone(), cx);
        cx.notify();
        let value = edit.input.value;
        if !success {
            return;
        }
        let target = parent.0.join(&value);
        match (edit.renamed, new_item) {
            (Some(renamed), _) if renamed != target => {
                let label = format!("Rename {} to {value}", file_name(&renamed));
                let edits = vec![FileEdit::Rename { old: resource(&renamed), new: resource(&target), options: FileEditOptions::default() }];
                let confirm = self.setting("explorer.confirmUndo") == json!("verbose");
                cx.spawn_in(window, async move |workbench, cx| {
                    if let Err(error) = apply_bulk_edit(&workbench, cx, edits, label, confirm).await {
                        notify_error(&workbench, cx, error).await;
                    }
                })
                .detach();
            }
            (None, Some(NewItem { is_dir, .. })) => self.create_explorer_item(target, is_dir || value.ends_with('/'), value, window, cx),
            _ => {}
        }
    }

    /// `onSuccess` of openExplorerAndCreate: a new folder is selected, a new file opens pinned; a failure offers
    /// to try again (`onErrorWithRetry`).
    fn create_explorer_item(&mut self, target: PathBuf, is_dir: bool, value: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |workbench, cx| {
            let options = FileEditOptions { folder: Some(is_dir), ..FileEditOptions::default() };
            let edits = vec![FileEdit::Create { new: resource(&target), options, contents: None }];
            match apply_bulk_edit(&workbench, cx, edits, format!("Create {value}"), true).await {
                Ok(()) => {
                    let _ = workbench.update_in(cx, |workbench, window, cx| match is_dir {
                        true => workbench.select_explorer_item(&target, window, cx),
                        false => workbench.open_file(target, true, window, cx),
                    });
                }
                Err(error) => {
                    let retry = workbench.update(cx, |workbench, cx| {
                        cx.notify();
                        workbench.prompt_notification(crate::notifications::Severity::Error, error, &["Retry"])
                    });
                    if let Ok(answer) = retry
                        && answer.await == Ok(Ok(json!(0)))
                    {
                        let _ = workbench.update_in(cx, |workbench, window, cx| workbench.create_explorer_item(target, is_dir, value, window, cx));
                    }
                }
            }
        })
        .detach();
    }

    /// The moves an operation ran, read from the renames of the edits that reverse it (`new` back to `old`).
    fn follow_explorer_moves(&mut self, undo: &FileUndo) {
        let Some(tree) = self.folder.as_mut() else { return };
        for edit in undo.operations.iter().flatten() {
            if let FileEdit::Rename { old, new, .. } = edit {
                tree.move_item(Path::new(&new.path), Path::new(&old.path));
            }
        }
    }

    /// `explorerService.select(resource, true)`: revealed, selected and focused.
    pub(super) fn select_explorer_item(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tree) = self.folder.as_mut() {
            tree.reveal(path);
        }
        window.focus(&self.explorer.focus.clone(), cx);
        cx.notify();
    }

    /// `moveFileToTrashHandler` and `deleteFileHandler`: the selection without the workspace folders.
    fn delete_explorer_items(&mut self, use_trash: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(tree) = self.folder.as_ref() else { return Outcome::done() };
        let stats: Vec<PathBuf> = tree.context(true, false).into_iter().filter(|stat| !tree.is_root(stat)).collect();
        if stats.is_empty() {
            return Outcome::done();
        }
        // `resources.distinctParents`: an item inside another deleted one goes with it.
        let distinct: Vec<PathBuf> = stats.iter().filter(|stat| !stats.iter().any(|other| other != *stat && stat.starts_with(other))).cloned().collect();
        let use_trash = use_trash && self.setting("files.enableTrash") == json!(true);
        cx.spawn_in(window, async move |workbench, cx| delete_files(workbench, cx, distinct, use_trash).await).detach();
        Outcome::done()
    }

    /// `copyFileHandler` and `cutFileHandler`: `setToCopy` with the selection.
    fn copy_explorer_items(&mut self, cut: bool, cx: &mut Context<Self>) -> Outcome {
        let Some(tree) = self.folder.as_mut() else { return Outcome::done() };
        let stats = tree.context(true, false);
        if !stats.is_empty() {
            crate::clipboard::write_resources(&stats);
            tree.clipboard = stats;
            tree.cut = cut;
            tree.file_copied = true;
            cx.notify();
        }
        Outcome::done()
    }

    /// `pasteFileHandler` for the files on the clipboard (`readResources`): each into the focused folder or the
    /// focused file's folder under a free name, or moved there after a cut.
    fn paste_explorer_items(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(tree) = self.folder.as_ref() else { return Outcome::done() };
        if self.explorer.edit.is_some() {
            return Outcome::done();
        }
        let element = tree.context(false, false).into_iter().next().unwrap_or_else(|| tree.workspace.folders[0].path.clone());
        let sources = crate::clipboard::distinct_parents(crate::clipboard::read_resources());
        let cut = tree.cut;
        let naming = self.incremental_naming();
        let mut pairs = Vec::new();
        for source in sources {
            if element != source && element.starts_with(&source) {
                return Outcome::error("File to paste is an ancestor of the destination folder");
            }
            let folder = match element == source {
                true => source.parent().expect("a pasted item has a parent").to_path_buf(),
                false => tree.folder_for(&element),
            };
            pairs.push((source, folder));
        }
        let confirm_undo = self.setting("explorer.confirmUndo");
        cx.spawn_in(window, async move |workbench, cx| {
            let mut targets = Vec::new();
            for (source, folder) in &pairs {
                let name = file_name(source);
                let target = match naming {
                    None if folder.join(&name).exists() && !confirm(&workbench, cx, PromptLevel::Warning, format!("A file or folder with the name '{name}' already exists in the destination folder. Do you want to replace it?"), None, "Replace").await => continue,
                    None => folder.join(&name),
                    // Moving to the same folder keeps the name.
                    Some(_) if cut => folder.join(&name),
                    Some(naming) => valid_paste_target(folder, &name, source.is_dir(), naming),
                };
                targets.push((source.clone(), target));
            }
            if targets.is_empty() {
                return;
            }
            let options = FileEditOptions { overwrite: Some(naming.is_none()), ..FileEditOptions::default() };
            let count = targets.len();
            let first = file_name(&targets[0].1);
            let (edits, label, confirm_before_undo) = match cut {
                true => (
                    targets.iter().map(|(source, target)| FileEdit::Rename { old: resource(source), new: resource(target), options: options.clone() }).collect(),
                    if count > 1 { format!("Move {count} files") } else { format!("Move {first}") },
                    confirm_undo == json!("verbose"),
                ),
                false => (
                    targets.iter().map(|(source, target)| FileEdit::Copy { old: resource(source), new: resource(target), options: FileEditOptions { copy: Some(true), ..options.clone() } }).collect(),
                    if count > 1 { format!("Paste {count} files") } else { format!("Paste {first}") },
                    confirm_undo != json!("light"),
                ),
            };
            let result = apply_bulk_edit(&workbench, cx, edits, label, confirm_before_undo).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                if cut && let Some(tree) = workbench.folder.as_mut() {
                    tree.clipboard.clear();
                    tree.cut = false;
                    tree.file_copied = false;
                }
                if result.is_ok() {
                    let target = targets[0].1.clone();
                    workbench.select_explorer_item(&target, window, cx);
                    if count == 1 && target.is_file() {
                        workbench.open_file(target, true, window, cx);
                        workbench.select_explorer_item(&targets[0].1, window, cx);
                    }
                }
            });
            if let Err(error) = result {
                notify_error(&workbench, cx, error).await;
            }
        })
        .detach();
        Outcome::done()
    }

    /// `UndoCommand` and `RedoCommand` with the Explorer focused: the last file edit, after asking when it was
    /// pushed with `confirmBeforeUndo` (undoRedoService.ts `_confirmAndContinueUndo`).
    pub(super) fn explorer_undo(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Outcome> {
        let direction = match command {
            "undo" => Direction::Undo,
            "redo" => Direction::Redo,
            _ => return None,
        };
        let focused = self.explorer.focus.is_focused(window) || self.explorer.input_focus.is_focused(window);
        if !focused || self.setting("explorer.enableUndo") != json!(true) {
            return None;
        }
        let element = self.explorer.undo.closest(UNDO_SOURCE, direction)?;
        let (id, label, confirm_first) = (element.id, element.label.clone(), element.confirm_before_undo && direction == Direction::Undo);
        cx.spawn_in(window, async move |workbench, cx| {
            if confirm_first && !confirm(&workbench, cx, PromptLevel::Info, format!("Would you like to undo '{label}'?"), None, "Yes").await {
                return;
            }
            let job = workbench.update(cx, |workbench, _| workbench.explorer.undo.begin(id, direction).map(|undo| workbench.host.run_files(FileJob::Reverse(undo))));
            let receiver = match job {
                Ok(Ok(receiver)) => receiver,
                Ok(Err(warning)) => return notify_error(&workbench, cx, warning).await,
                Err(_) => return,
            };
            let result = receiver.await.unwrap_or_else(|_| Err(files::FileError { name: "Error".into(), message: "Canceled".into(), operation: None }));
            let failed = workbench.update(cx, |workbench, cx| {
                cx.notify();
                match result {
                    Ok(undo) => {
                        workbench.follow_explorer_moves(&undo);
                        workbench.explorer.undo.finish(id, undo);
                        None
                    }
                    Err(error) => {
                        workbench.explorer.undo.fail(id);
                        Some(error.message)
                    }
                }
            });
            if let Ok(Some(error)) = failed {
                notify_error(&workbench, cx, error).await;
            }
        })
        .detach();
        Some(Outcome::done())
    }
}

/// The Explorer items a drag carries (explorerViewer.ts `FileDragAndDrop`); it draws the drag image
/// (dnd.css `.monaco-drag-image`) with `getDragLabel`.
#[derive(Clone)]
pub struct DraggedItems {
    pub paths: Vec<PathBuf>,
    background: Rgba,
    foreground: Rgba,
    outline: Option<Rgba>,
}

impl Render for DraggedItems {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let label = match self.paths.as_slice() {
            [path] => file_name(path),
            paths => paths.len().to_string(),
        };
        div()
            .px(px(7.))
            .py(px(1.))
            .rounded(px(10.))
            .text_size(px(12.))
            .bg(self.background)
            .text_color(self.foreground)
            .when_some(self.outline, |image, color| image.border_1().border_color(color))
            .child(label)
    }
}

impl Workbench {
    /// The items a drag on `path` carries: the selection when it holds `path`, else `path` alone; workspace
    /// folders do not move.
    pub(super) fn explorer_drag(&self, path: &Path) -> Option<DraggedItems> {
        let tree = self.folder.as_ref()?;
        if tree.is_root(path) || self.setting("explorer.enableDragAndDrop") != json!(true) {
            return None;
        }
        let paths = match tree.selected.iter().any(|selected| selected == path) {
            true => tree.context(true, false).into_iter().filter(|stat| !tree.is_root(stat)).collect(),
            false => vec![path.to_path_buf()],
        };
        Some(DraggedItems {
            paths,
            background: self.color("list.activeSelectionBackground"),
            foreground: self.foreground_or("list.activeSelectionForeground"),
            outline: self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")),
        })
    }

    /// `handleDragOver` with the pointer over `over`, or over the empty area below the rows: the folder it drops into
    /// (a file drops into its folder, the empty area into the last workspace folder), unless that would move an item
    /// onto itself, into its own folder or below itself.
    pub(super) fn explorer_drag_over(&mut self, over: Option<&Path>, items: &DraggedItems, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.folder.as_ref() else { return };
        let copy = window.modifiers().alt;
        let target = match over {
            Some(path) => tree.folder_for(path),
            None if !copy && items.paths.iter().all(|source| source.parent().is_some_and(|parent| tree.is_root(parent))) => {
                return self.set_explorer_drop_target(None, over, window, cx);
            }
            None => last_root(tree),
        };
        let rejected = items.paths.iter().any(|source| *source == target || (!copy && source.parent() == Some(&target)) || target.starts_with(source));
        self.set_explorer_drop_target((!rejected).then_some(target), over, window, cx);
    }

    /// `handleDragOver` for files dragged in from another app (`NativeDragAndDropData`): always a copy, into the
    /// folder under the pointer or the last workspace folder.
    pub(super) fn explorer_external_drag_over(&mut self, over: Option<&Path>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.folder.as_ref() else { return };
        let target = match over {
            _ if self.setting("explorer.enableDragAndDrop") != json!(true) => None,
            Some(path) => Some(tree.folder_for(path)),
            None => Some(last_root(tree)),
        };
        self.set_explorer_drop_target(target, over, window, cx);
    }

    /// A collapsed folder held under the pointer for 500ms expands (`autoExpand`).
    fn set_explorer_drop_target(&mut self, target: Option<PathBuf>, over: Option<&Path>, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer.drop_target == target {
            return;
        }
        self.explorer.drop_target = target.clone();
        self.explorer.expand_timer = None;
        let Some(tree) = self.folder.as_ref() else { return };
        if let Some(folder) = target.filter(|folder| Some(folder.as_path()) == over && !tree.is_expanded(folder)) {
            self.explorer.expand_timer = Some(cx.spawn_in(window, async move |workbench, cx| {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    if workbench.explorer.drop_target.as_ref() == Some(&folder)
                        && let Some(tree) = workbench.folder.as_mut()
                    {
                        tree.expand(&folder);
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }

    /// iconLabel.ts with a label array: an `a.label-name` per folder of a compact row, a half-opaque `/` 2px from its
    /// neighbors. explorerviewlet.css underlines the hovered name and, in the focused row, the current one; a name is
    /// the drop target on its own. Pressing a name makes it current (`renderCompressedElements` mousedown).
    pub(super) fn compressed_label(&self, folders: &[(PathBuf, String)], navigation: &Navigation, row_focused: bool, drop_target: Option<&PathBuf>, cx: &mut Context<Self>) -> Div {
        let row = navigation.items.last().expect("a compact row has folders").clone();
        let drop_background = self.color("list.dropBackground");
        div().flex().children(folders.iter().enumerate().map(|(index, (folder, name))| {
            let is_last = index == folders.len() - 1;
            let current = (row.clone(), index);
            let menu_target = (row.clone(), index);
            let over = folder.clone();
            let external_over = folder.clone();
            div()
                .id(("explorer-compressed-name", index))
                .flex()
                .group("explorer-compressed-name")
                .child(
                    div()
                        .rounded(px(3.))
                        .when(row_focused && index == navigation.index, |name| name.underline())
                        .group_hover("explorer-compressed-name", |name| name.underline())
                        .when(drop_target == Some(folder), |name| name.bg(drop_background))
                        .child(name.clone()),
                )
                .when(!is_last, |name| name.child(div().mx(px(2.)).opacity(0.5).child("/")))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, _, cx| workbench.set_compressed_current(&current.0, current.1, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.set_compressed_current(&menu_target.0, menu_target.1, cx);
                    workbench.show_explorer_context_menu(Some(&menu_target.0), event.position, window, cx);
                }))
                .when(!is_last, |name| {
                    name.on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<DraggedItems>, window, cx| {
                        if event.bounds.contains(&event.event.position) {
                            cx.stop_propagation();
                            let items = event.drag(cx).clone();
                            workbench.explorer_drag_over(Some(&over), &items, window, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<ExternalPaths>, window, cx| {
                        if event.bounds.contains(&event.event.position) {
                            cx.stop_propagation();
                            workbench.explorer_external_drag_over(Some(&external_over), window, cx);
                        }
                    }))
                })
        }))
    }

    fn set_compressed_current(&mut self, row: &Path, index: usize, cx: &mut Context<Self>) {
        let tree = self.folder.as_mut().expect("rows exist only with a folder");
        if let Some(mut navigation) = tree.navigation_at(row) {
            navigation.set_index(index);
            tree.set_navigation(&navigation);
            cx.notify();
        }
    }

    /// `handleExplorerDrop`: Alt copies, otherwise the items move after `explorer.confirmDragAndDrop` asks.
    pub(super) fn explorer_drop(&mut self, items: &DraggedItems, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer.expand_timer = None;
        let Some(target) = self.explorer.drop_target.take() else { return };
        cx.notify();
        let copy = window.modifiers().alt;
        let sources: Vec<PathBuf> = items.paths.iter().filter(|stat| !items.paths.iter().any(|other| other != *stat && stat.starts_with(other))).cloned().collect();
        let confirm_move = !copy && self.setting("explorer.confirmDragAndDrop") == json!(true);
        let naming = self.incremental_naming();
        let confirm_undo = self.setting("explorer.confirmUndo");
        let target_name = file_name(&target);
        cx.spawn_in(window, async move |workbench, cx| {
            if confirm_move {
                let (message, detail) = match sources.as_slice() {
                    [source] => (format!("Are you sure you want to move '{}' into '{target_name}'?", file_name(source)), None),
                    _ => (format!("Are you sure you want to move the following {} files into '{target_name}'?", sources.len()), Some(file_actions::file_names_message(&sources.iter().map(|source| file_name(source)).collect::<Vec<_>>()))),
                };
                if !confirm_or_stop_asking(&workbench, cx, PromptLevel::Info, message, detail, "Move", "explorer.confirmDragAndDrop").await {
                    return;
                }
            }
            let suffix = label_suffix(&sources);
            let result = match copy {
                true => {
                    let mut edits = Vec::new();
                    let mut targets = Vec::new();
                    for source in &sources {
                        let name = file_name(source);
                        let new = match naming {
                            None if target.join(&name).exists() && !confirm(&workbench, cx, PromptLevel::Warning, overwrite_message(&name), Some("This action is irreversible!".into()), "Replace").await => continue,
                            None => target.join(&name),
                            Some(naming) => valid_paste_target(&target, &name, source.is_dir(), naming),
                        };
                        let options = FileEditOptions { copy: Some(true), overwrite: Some(naming.is_none()), ..FileEditOptions::default() };
                        edits.push(FileEdit::Copy { old: resource(source), new: resource(&new), options });
                        targets.push(new);
                    }
                    let result = apply_bulk_edit(&workbench, cx, edits, format!("Copy {suffix}"), confirm_undo != json!("light")).await;
                    if result.is_ok() {
                        let _ = workbench.update_in(cx, |workbench, window, cx| {
                            for target in targets.into_iter().filter(|target| target.is_file()) {
                                workbench.open_file(target, true, window, cx);
                            }
                        });
                    }
                    result
                }
                false => {
                    let moves: Vec<(PathBuf, PathBuf)> = sources.iter().map(|source| (source.clone(), target.join(file_name(source)))).collect();
                    let existing: Vec<String> = moves.iter().filter(|(_, new)| new.exists()).map(|(_, new)| file_name(new)).collect();
                    // FILE_MOVE_CONFLICT: replacing asks first (getMultipleFilesOverwriteConfirm).
                    let overwrite = !existing.is_empty();
                    if overwrite {
                        let (message, detail) = match existing.as_slice() {
                            [name] => (overwrite_message(name), "This action is irreversible!".to_string()),
                            names => (
                                format!("The following {} files and/or folders already exist in the destination folder. Do you want to replace them?", names.len()),
                                format!("{}\nThis action is irreversible!", file_actions::file_names_message(names)),
                            ),
                        };
                        if !confirm(&workbench, cx, PromptLevel::Warning, message, Some(detail), "Replace").await {
                            return;
                        }
                    }
                    let options = FileEditOptions { overwrite: Some(overwrite), ..FileEditOptions::default() };
                    let edits = moves.iter().map(|(old, new)| FileEdit::Rename { old: resource(old), new: resource(new), options: options.clone() }).collect();
                    apply_bulk_edit(&workbench, cx, edits, format!("Move {suffix}"), confirm_undo == json!("verbose")).await
                }
            };
            if let Err(error) = result {
                notify_error(&workbench, cx, error).await;
            }
        })
        .detach();
    }

    /// The tree below its last row. A drop there has no target row (explorerViewer.ts `drop`), so it goes into the
    /// last workspace folder; with no root rows that folder is the whole list, highlighted here too.
    pub(super) fn explorer_empty_area(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let whole_list_targeted = cx.has_active_drag()
            && self.folder.as_ref().is_some_and(|tree| !tree.shows_roots() && self.explorer.drop_target.as_ref().is_some_and(|target| tree.is_root(target)));
        div()
            .id("explorer-empty-area")
            .flex_1()
            .when(whole_list_targeted, |area| area.bg(self.color("list.dropBackground")))
            .on_drag_move(cx.listener(|workbench, event: &DragMoveEvent<DraggedItems>, window, cx| {
                if event.bounds.contains(&event.event.position) {
                    let items = event.drag(cx).clone();
                    workbench.explorer_drag_over(None, &items, window, cx);
                }
            }))
            .on_drag_move(cx.listener(|workbench, event: &DragMoveEvent<ExternalPaths>, window, cx| {
                if event.bounds.contains(&event.event.position) {
                    workbench.explorer_external_drag_over(None, window, cx);
                }
            }))
            .on_drop(cx.listener(|workbench, items: &DraggedItems, window, cx| workbench.explorer_drop(items, window, cx)))
            .on_drop(cx.listener(|workbench, paths: &ExternalPaths, window, cx| workbench.explorer_import(paths, window, cx)))
    }

    /// fileImportExport.ts `ExternalFileImport.doImport`: folders dropped on a workspace folder ask whether to copy
    /// them, then each dropped item whose name the target already has asks to replace it, and the rest copy in.
    pub(super) fn explorer_import(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer.expand_timer = None;
        let Some(target) = self.explorer.drop_target.take() else { return };
        let Some(tree) = self.folder.as_ref() else { return };
        cx.notify();
        window.activate_window();
        let folders: Vec<PathBuf> = paths.paths().iter().filter(|path| path.is_dir()).cloned().collect();
        let ask_about_folders = !folders.is_empty() && tree.is_root(&target);
        let resources = paths.paths().to_vec();
        let confirm_undo = self.setting("explorer.confirmUndo");
        let auto_open = self.setting("explorer.autoOpenDroppedFile") == json!(true);
        cx.spawn_in(window, async move |workbench, cx| {
            if ask_about_folders {
                // Every dropped folder is a `file` resource, as every workspace folder, so Add is always offered.
                let (message, add, copy) = match folders.as_slice() {
                    [folder] => {
                        let name = file_name(folder);
                        (format!("Do you want to copy '{name}' or add '{name}' as a folder to the workspace?"), "Add Folder to Workspace", "Copy Folder")
                    }
                    _ => ("Do you want to copy the folders or add the folders to the workspace?".to_string(), "Add Folders to Workspace", "Copy Folders"),
                };
                match prompt(&workbench, cx, PromptLevel::Info, message, &[add, copy, "Cancel"]).await {
                    Some(0) => {
                        // shortcut: workspaceEditingService.addFolders is not ported (port/notes.md milestone 12); upgrade with it.
                        notify_error(&workbench, cx, "Adding folders to the workspace is not supported yet.".to_string()).await;
                        return;
                    }
                    Some(1) => {}
                    _ => return,
                }
            }
            if let Err(error) = import_resources(&workbench, cx, &target, resources, confirm_undo != json!("light"), auto_open).await {
                notify_error(&workbench, cx, error).await;
            }
        })
        .detach();
    }

    fn incremental_naming(&self) -> Option<IncrementalNaming> {
        match self.setting("explorer.incrementalNaming").as_str() {
            Some("smart") => Some(IncrementalNaming::Smart),
            Some("disabled") => None,
            _ => Some(IncrementalNaming::Simple),
        }
    }
}

/// `getFileOrFolderLabelSuffix`.
fn label_suffix(items: &[PathBuf]) -> String {
    match items {
        [item] => file_name(item),
        _ if items.iter().all(|item| item.is_dir()) => format!("{} folders", items.len()),
        _ if items.iter().all(|item| !item.is_dir()) => format!("{} files", items.len()),
        _ => format!("{} files and folders", items.len()),
    }
}

/// A validation message's markdown: its only markup is `**bold**` (`renderMarkdown` draws it as `<strong>`).
fn markdown_bold(message: &str) -> StyledText {
    let mut text = String::new();
    let mut bold = Vec::new();
    for (index, part) in message.split("**").enumerate() {
        if index % 2 == 1 {
            bold.push((text.len()..text.len() + part.len(), HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() }));
        }
        text.push_str(part);
    }
    StyledText::new(text).with_highlights(bold)
}

/// `getFileOverwriteConfirm`.
fn overwrite_message(name: &str) -> String {
    format!("A file or folder with the name '{name}' already exists in the destination folder. Do you want to replace it?")
}

/// `deleteFiles`: asks about unsaved changes, then for confirmation (`explorer.confirmDelete`), then deletes;
/// a failure offers to delete permanently, or to retry.
async fn delete_files(workbench: WeakEntity<Workbench>, cx: &mut AsyncWindowContext, items: Vec<PathBuf>, mut use_trash: bool) {
    let deleted: Vec<Deleted> = items.iter().map(|path| Deleted { name: file_name(path), is_dir: path.is_dir() }).collect();
    let mut skip_confirm = false;
    let mut ignore_if_not_exists = false;
    loop {
        let primary = if use_trash { "Move to Trash" } else { "Delete" };
        let Ok((dirty, confirm_delete)) = workbench.update(cx, |workbench, _| {
            let dirty = workbench.editors.editors.iter().filter(|editor| editor.is_dirty() && items.iter().any(|item| editor.document.path.starts_with(item))).count();
            (dirty, workbench.setting("explorer.confirmDelete") != json!(false))
        }) else {
            return;
        };
        if dirty > 0 {
            let message = match (items.len() > 1, deleted[0].is_dir, dirty) {
                (true, _, _) => "You are deleting files with unsaved changes. Do you want to continue?".to_string(),
                (false, true, 1) => format!("You are deleting a folder {} with unsaved changes in 1 file. Do you want to continue?", deleted[0].name),
                (false, true, count) => format!("You are deleting a folder {} with unsaved changes in {count} files. Do you want to continue?", deleted[0].name),
                (false, false, _) => format!("You are deleting {} with unsaved changes. Do you want to continue?", deleted[0].name),
            };
            if !confirm(&workbench, cx, PromptLevel::Warning, message, Some("Your changes will be lost if you don't save them.".into()), primary).await {
                return;
            }
            skip_confirm = true;
        }
        if !skip_confirm && confirm_delete {
            let (message, detail) = file_actions::delete_confirmation(&deleted, use_trash);
            let confirmed = match use_trash {
                true => confirm_or_stop_asking(&workbench, cx, PromptLevel::Info, message, Some(detail), primary, "explorer.confirmDelete").await,
                false => confirm(&workbench, cx, PromptLevel::Warning, message, Some(detail), primary).await,
            };
            if !confirmed {
                return;
            }
        }
        let edits = items
            .iter()
            .zip(&deleted)
            .map(|(path, item)| {
                let options = FileEditOptions {
                    recursive: Some(true),
                    folder: Some(item.is_dir),
                    ignore_if_not_exists: Some(ignore_if_not_exists),
                    skip_trash_bin: Some(!use_trash),
                    max_size: Some(MAX_UNDO_FILE_SIZE),
                    ..FileEditOptions::default()
                };
                FileEdit::Delete { old: resource(path), options, undoes_create: false }
            })
            .collect();
        let label = if items.len() > 1 { format!("Delete {} files", items.len()) } else { format!("Delete {}", deleted[0].name) };
        let Err(error) = apply_bulk_edit(&workbench, cx, edits, label, false).await else { return };
        let (message, detail, retry) = match use_trash {
            true => ("Failed to delete using the Trash. Do you want to permanently delete instead?".to_string(), Some(file_actions::delete_detail(&deleted).to_string()), "Delete Permanently"),
            false => (error, None, "Retry"),
        };
        if !confirm(&workbench, cx, PromptLevel::Warning, message, detail, retry).await {
            return;
        }
        use_trash = false;
        skip_confirm = true;
        ignore_if_not_exists = true;
    }
}

/// `explorerService.applyBulkEdit`: the edits run with the extension host's participants and their undo joins
/// the workbench undo stack under the Explorer's source.
async fn apply_bulk_edit(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, edits: Vec<FileEdit>, label: String, confirm_before_undo: bool) -> Result<(), String> {
    let receiver = workbench.update(cx, |workbench, _| workbench.host.run_files(FileJob::Apply(edits))).map_err(|error| error.to_string())?;
    let undo = receiver.await.map_err(|_| "Canceled".to_string())?.map_err(|error| error.message)?;
    workbench
        .update(cx, |workbench, cx| {
            workbench.follow_explorer_moves(&undo);
            let resources = undo.resources().iter().map(|resource| (resource.path.to_lowercase(), resource.path.clone())).collect();
            workbench.explorer.undo.push(label, resources, UNDO_SOURCE, confirm_before_undo, undo);
            cx.notify();
        })
        .map_err(|error| error.to_string())
}

/// `dialogService.confirm`: true for the primary button; with dialogs skipped, confirmed.
pub(super) async fn confirm(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, level: PromptLevel, message: String, detail: Option<String>, primary: &str) -> bool {
    let answer = workbench.update_in(cx, |workbench, window, cx| match workbench.skip_dialogs {
        true => None,
        false => Some(window.prompt(level, &message, detail.as_deref(), &[primary, "Cancel"], cx)),
    });
    match answer {
        Ok(Some(answer)) => answer.await == Ok(0),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// `dialogService.prompt`: the index of the pressed button; with dialogs skipped, the first.
async fn prompt(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, level: PromptLevel, message: String, buttons: &[&str]) -> Option<usize> {
    let answer = workbench.update_in(cx, |workbench, window, cx| match workbench.skip_dialogs {
        true => None,
        false => Some(window.prompt(level, &message, None, buttons, cx)),
    });
    match answer {
        Ok(Some(answer)) => answer.await.ok(),
        Ok(None) => Some(0),
        Err(_) => None,
    }
}

/// `ExternalFileImport.importResources`: a dropped item missing on disk is counted, one whose name `target` already
/// has (in any case, the disk is not PathCaseSensitive) asks to replace it, and the rest copy in as one undoable
/// edit. A single dropped file opens pinned with `explorer.autoOpenDroppedFile`.
async fn import_resources(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, target: &Path, resources: Vec<PathBuf>, confirm_before_undo: bool, auto_open: bool) -> Result<(), String> {
    let target_names: std::collections::HashSet<String> = std::fs::read_dir(target).map_err(|error| error.to_string())?.flatten().map(|child| child.file_name().to_string_lossy().to_lowercase()).collect();
    let mut inaccessible = 0;
    let mut imported = Vec::new();
    for resource in resources {
        if !resource.exists() {
            inaccessible += 1;
            continue;
        }
        let name = file_name(&resource);
        if target_names.contains(&name.to_lowercase()) && !confirm(workbench, cx, PromptLevel::Warning, overwrite_message(&name), Some("This action is irreversible!".into()), "Replace").await {
            continue;
        }
        imported.push(resource);
    }
    if inaccessible > 0 {
        let message = match inaccessible {
            1 => "The dropped file could not be accessed for import.",
            _ => "Some or all of the dropped files could not be accessed for import.",
        };
        notify_error(workbench, cx, message.to_string()).await;
    }
    if imported.is_empty() {
        return Ok(());
    }
    let label = match imported.as_slice() {
        [resource] => format!("Import {}", file_name(resource)),
        _ => format!("Import {} resources", imported.len()),
    };
    let options = FileEditOptions { copy: Some(true), overwrite: Some(true), ..FileEditOptions::default() };
    let targets: Vec<PathBuf> = imported.iter().map(|resource| target.join(file_name(resource))).collect();
    let edits = imported.iter().zip(&targets).map(|(old, new)| FileEdit::Copy { old: resource(old), new: resource(new), options: options.clone() }).collect();
    apply_bulk_edit(workbench, cx, edits, label, confirm_before_undo).await?;
    if let [target] = targets.as_slice()
        && auto_open
        && target.is_file()
    {
        let target = target.clone();
        let _ = workbench.update_in(cx, |workbench, window, cx| workbench.open_file(target, true, window, cx));
    }
    Ok(())
}

/// `dialogService.confirm` with the "Do not ask me again" checkbox; a confirmation with the box checked
/// turns `setting` off. With dialogs skipped, confirmed.
async fn confirm_or_stop_asking(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, level: PromptLevel, message: String, detail: Option<String>, primary: &str, setting: &str) -> bool {
    let answer = workbench.update_in(cx, |workbench, window, cx| match workbench.skip_dialogs {
        true => None,
        false => Some(dialog::show_message_box(window, level, &message, detail.as_deref(), &[primary, "Cancel"], Some("Do not ask me again"), cx)),
    });
    let checked = match answer {
        Ok(Some(answer)) => match answer.await {
            Ok((0, checked)) => checked,
            _ => return false,
        },
        Ok(None) => return true,
        Err(_) => return false,
    };
    if checked {
        let _ = workbench.update(cx, |workbench, _| workbench.update_setting(setting, json!(false)));
    }
    true
}

/// `notificationService.error`.
async fn notify_error(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, message: String) {
    let _ = workbench.update(cx, |workbench, cx| {
        workbench.notify_error(message);
        cx.notify();
    });
}

/// `findValidPasteFileTarget`: the name, incremented until nothing in the folder has it.
fn valid_paste_target(folder: &Path, name: &str, is_dir: bool, naming: IncrementalNaming) -> PathBuf {
    let mut name = name.to_string();
    while folder.join(&name).exists() {
        name = file_actions::increment_file_name(&name, is_dir, naming);
    }
    folder.join(name)
}

/// Where a drop below the rows goes: `explorerService.roots`' last.
fn last_root(tree: &crate::explorer::FileTree) -> PathBuf {
    tree.workspace.folders.last().expect("a workspace has a folder").path.clone()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

fn resource(path: &Path) -> Resource {
    Resource::file(path.to_string_lossy())
}

/// `workbench.tree.indent` per level: clamped to 0..40 as abstractTree.ts `updateOptions` does, 8 when not a number.
fn tree_indent(value: &Value) -> f32 {
    value.as_f64().map_or(8., |indent| indent.clamp(0., 40.)) as f32
}
