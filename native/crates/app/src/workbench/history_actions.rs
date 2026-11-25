//! historyService.ts in the workbench: the navigation stacks fed by editor and selection changes, Go Back and
//! Go Forward with the edit and navigation location variants, Reopen Closed Editor, and the recently used
//! editor commands (editorActions.ts).

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{Selection, VerticalReveal};
use crate::navigation::{Entry, Filter, Reason, Stacks};

/// `RECENTLY_CLOSED_EDITORS` limit (`MAX_RECENTLY_CLOSED_EDITORS`).
const MAX_RECENTLY_CLOSED: usize = 20;

/// `IRecentlyClosedEditor`: a closed file editor by its resource, group and tab index.
pub(super) struct ClosedEditor {
    group: u32,
    path: PathBuf,
    index: usize,
}

#[derive(Default)]
pub(super) struct History {
    /// The stacks of each `workbench.editor.navigationScope` key: none for `default`, the group for `editorGroup`,
    /// the group and resource for `editor`.
    stacks: HashMap<(Option<u32>, Option<PathBuf>), Stacks>,
    scope: String,
    closed: Vec<ClosedEditor>,
    /// The last editor a change was recorded for, so only a different one counts as an editor change.
    last_editor: Option<(u32, String, PathBuf)>,
    /// `recentlyUsedEditorsStack` and its index while Open Previous/Next Recently Used Editor runs.
    recently_used: Option<(Vec<(u32, String, PathBuf)>, usize)>,
    navigating_recently_used: bool,
}

impl Workbench {
    pub(super) fn register_history_actions(&mut self) {
        let walks: [(&str, Filter, fn(&mut crate::navigation::Stack) -> Option<Entry>); 12] = [
            ("workbench.action.navigateBack", Filter::None, |stack| stack.go_back()),
            ("workbench.action.navigateForward", Filter::None, |stack| stack.go_forward()),
            ("workbench.action.navigateLast", Filter::None, |stack| stack.go_previous()),
            ("workbench.action.navigateBackInEditLocations", Filter::Edits, |stack| stack.go_back()),
            ("workbench.action.navigateForwardInEditLocations", Filter::Edits, |stack| stack.go_forward()),
            ("workbench.action.navigatePreviousInEditLocations", Filter::Edits, |stack| stack.go_previous()),
            ("workbench.action.navigateToLastEditLocation", Filter::Edits, |stack| stack.go_last()),
            ("workbench.action.navigateBackInNavigationLocations", Filter::Navigation, |stack| stack.go_back()),
            ("workbench.action.navigateForwardInNavigationLocations", Filter::Navigation, |stack| stack.go_forward()),
            ("workbench.action.navigatePreviousInNavigationLocations", Filter::Navigation, |stack| stack.go_previous()),
            ("workbench.action.navigateToLastNavigationLocation", Filter::Navigation, |stack| stack.go_last()),
            ("workbench.action.navigateLastInNavigationLocations", Filter::Navigation, |stack| stack.go_last()),
        ];
        for (id, filter, walk) in walks {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.navigate(filter, walk, window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.reopenClosedEditor", |workbench, _, window, cx| {
            workbench.reopen_closed_editor(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.clearEditorHistory", |workbench, _, window, cx| {
            if workbench.skip_dialogs {
                workbench.clear_history();
                return Outcome::done();
            }
            let answer = window.prompt(PromptLevel::Warning, "Do you want to clear the history of recently opened editors?", Some("This action is irreversible!"), &["Clear", "Cancel"], cx);
            cx.spawn(async move |workbench, cx| {
                if answer.await == Ok(0) {
                    let _ = workbench.update(cx, |workbench, _| workbench.clear_history());
                }
            })
            .detach();
            Outcome::done()
        });
        let recently_used = [
            ("workbench.action.openNextRecentlyUsedEditor", false, false),
            ("workbench.action.openPreviousRecentlyUsedEditor", true, false),
            ("workbench.action.openNextRecentlyUsedEditorInGroup", false, true),
            ("workbench.action.openPreviousRecentlyUsedEditorInGroup", true, true),
        ];
        for (id, previous, in_group) in recently_used {
            self.commands.register(id, move |workbench, _, window, cx| {
                workbench.open_recently_used(previous, in_group, window, cx);
                Outcome::done()
            });
        }
    }

    /// historyService.ts `updateContextKeys`, for the stacks of the active group and editor.
    pub(super) fn history_context(&self, context: &mut HashMap<String, Value>) {
        let stacks = self.history.stacks.get(&self.navigation_key());
        let can = |filter: Filter, check: fn(&crate::navigation::Stack) -> bool| stacks.is_some_and(|stacks| check(stacks.stack(filter)));
        context.insert("canNavigateBack".into(), json!(can(Filter::None, |stack| stack.can_go_back())));
        context.insert("canNavigateForward".into(), json!(can(Filter::None, |stack| stack.can_go_forward())));
        context.insert("canNavigateBackInNavigationLocations".into(), json!(can(Filter::Navigation, |stack| stack.can_go_back())));
        context.insert("canNavigateForwardInNavigationLocations".into(), json!(can(Filter::Navigation, |stack| stack.can_go_forward())));
        context.insert("canNavigateToLastNavigationLocation".into(), json!(can(Filter::Navigation, |stack| stack.can_go_last())));
        context.insert("canNavigateBackInEditLocations".into(), json!(can(Filter::Edits, |stack| stack.can_go_back())));
        context.insert("canNavigateForwardInEditLocations".into(), json!(can(Filter::Edits, |stack| stack.can_go_forward())));
        context.insert("canNavigateToLastEditLocation".into(), json!(can(Filter::Edits, |stack| stack.can_go_last())));
        context.insert("canReopenClosedEditor".into(), json!(!self.history.closed.is_empty()));
    }

    pub(super) fn can_navigate(&self, back: bool) -> bool {
        let Some(stacks) = self.history.stacks.get(&self.navigation_key()) else { return false };
        if back { stacks.selections.can_go_back() } else { stacks.selections.can_go_forward() }
    }

    /// `getStack`: the key of `workbench.editor.navigationScope`; changing the setting clears every stack.
    fn navigation_key(&self) -> (Option<u32>, Option<PathBuf>) {
        let editor = self.editors.active_tab().map(|editor| editor.document().path.clone());
        match self.history.scope.as_str() {
            "editorGroup" => (Some(self.editors.id), None),
            "editor" => (Some(self.editors.id), editor),
            _ => (None, None),
        }
    }

    /// The active editor as a stack entry: a text editor with its selection's start line and cursor.
    fn active_entry(&self) -> Option<Entry> {
        let tab = self.editors.active_tab()?;
        let document = tab.document();
        let selection = self.editors.active().map(|editor| {
            let selection = editor.selection();
            (selection.start(), selection.head)
        });
        Some(Entry { group: self.editors.id, scheme: document.scheme.clone(), path: document.path.clone(), selection })
    }

    /// `onDidActiveEditorChange` and `onDidChangeSelection` of the active editor pane: `reason` is `None` for an
    /// editor change.
    pub(super) fn record_navigation(&mut self, reason: Option<Reason>) {
        let scope = match self.setting("workbench.editor.navigationScope").as_str() {
            Some(scope) => scope.to_string(),
            None => "default".into(),
        };
        if scope != self.history.scope {
            self.history.stacks.clear();
            self.history.scope = scope;
        }
        let Some(entry) = self.active_entry() else {
            self.history.last_editor = None;
            return;
        };
        let editor = (entry.group, entry.scheme.clone(), entry.path.clone());
        if reason.is_none() && self.history.last_editor.as_ref() == Some(&editor) {
            return;
        }
        if reason.is_none() && !self.history.navigating_recently_used {
            self.history.recently_used = None;
        }
        self.history.last_editor = Some(editor);
        let key = self.navigation_key();
        self.history.stacks.entry(key).or_default().notify(entry, reason);
    }

    /// `EditorNavigationStack.navigate`: opens the entry's editor in its group (or the active one when the group
    /// closed) with its selection, while the stack takes no entries.
    fn navigate(&mut self, filter: Filter, walk: fn(&mut crate::navigation::Stack) -> Option<Entry>, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.navigation_key();
        let at = self.active_entry();
        let Some(stacks) = self.history.stacks.get_mut(&key) else { return };
        let stack = stacks.stack_mut(filter);
        // `maybeGoCurrent`: the filtered stacks first go to the entry they stand on.
        let target = match (filter, at.as_ref().and_then(|at| stack.current_if_away(at))) {
            (Filter::Edits | Filter::Navigation, Some(current)) => Some(current),
            _ => walk(stack),
        };
        let Some(entry) = target else { return };
        stacks.navigating = Some(filter);
        self.open_entry(&entry, window, cx);
        for stacks in self.history.stacks.values_mut() {
            stacks.navigating = None;
        }
    }

    fn open_entry(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        if self.groups_in_grid_order().contains(&entry.group) {
            self.focus_group(entry.group, window, cx);
        }
        let shown = (0..self.editors.editors.len()).find(|&index| self.editors.editors[index].document().is(&entry.scheme, &entry.path));
        match shown {
            Some(index) => {
                self.editors.activate(index);
                self.editors_changed(cx);
            }
            None if entry.scheme == "file" => self.open_file(entry.path.clone(), true, window, cx),
            None => return,
        }
        window.focus(&self.editor_focus, cx);
        let Some((start, head)) = entry.selection else { return };
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.document().is(&entry.scheme, &entry.path)) else { return };
        let lines = editor.document().lines.len();
        let clamp = |position: crate::editor::Position| crate::editor::Position::new(position.line.min(lines - 1), position.column);
        let anchor = if start == head { clamp(head) } else { clamp(start) };
        editor.set_selections(vec![Selection { anchor, head: clamp(head) }]);
        editor.reveal_range(clamp(head), clamp(head), VerticalReveal::CenterIfOutsideViewport, viewport);
        self.host.send_selection(self.editors.active().expect("just selected"), "code.navigation");
        self.record_navigation(Some(Reason::Programmatic));
        self.editor_scrolled(cx);
    }

    /// `remove`: entries of a deleted file (`onDidFilesChange`) or a removed group (`onDidRemoveGroup`) go.
    pub(super) fn remove_from_navigation(&mut self, gone: impl Fn(&Entry) -> bool) {
        for stacks in self.history.stacks.values_mut() {
            stacks.remove(&gone);
        }
    }

    /// A deleted file leaves the stacks and the recently closed editors (`onDidFilesChange` DELETED).
    pub(super) fn file_deleted_from_history(&mut self, path: &std::path::Path) {
        self.remove_from_navigation(|entry| entry.scheme == "file" && entry.path.starts_with(path));
        self.history.closed.retain(|closed| !closed.path.starts_with(path));
    }

    /// historyService.ts `onEditorClosed`: a closed file editor can be reopened (not one replaced or moved).
    pub(super) fn remember_closed_editor(&mut self, group: u32, path: PathBuf, index: usize) {
        self.history.closed.retain(|closed| closed.path != path);
        self.history.closed.push(ClosedEditor { group, path, index });
        if self.history.closed.len() > MAX_RECENTLY_CLOSED {
            self.history.closed.remove(0);
        }
    }

    /// `reopenLastClosedEditor`: the last closed editor not open in the active group, pinned, at its old index.
    fn reopen_closed_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(closed) = self.history.closed.pop() {
            if self.editors.find(&closed.path).is_some() || !closed.path.exists() {
                continue;
            }
            if closed.group != self.editors.id && self.groups_in_grid_order().contains(&closed.group) {
                self.focus_group(closed.group, window, cx);
            }
            self.open_file(closed.path.clone(), true, window, cx);
            if let Some(from) = self.editors.find(&closed.path) {
                self.editors.move_editor(from, closed.index);
                self.editors_changed(cx);
            }
            return;
        }
    }

    /// `clear`: the stacks, the recently closed editors and the editor history of Quick Open.
    fn clear_history(&mut self) {
        self.history.stacks.clear();
        self.history.closed.clear();
        self.history.recently_used = None;
        self.clear_quick_open_history();
    }

    /// `openNextRecentlyUsedEditor` / `openPreviousRecentlyUsedEditor`: a snapshot of the editors by recency
    /// (every group's, or the active group's), walked while these commands run.
    fn open_recently_used(&mut self, previous: bool, in_group: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (snapshot, index) = match self.history.recently_used.take() {
            Some(found) => found,
            None => (self.editors_by_recency(in_group), 0),
        };
        let index = if previous { (index + 1).min(snapshot.len().saturating_sub(1)) } else { index.saturating_sub(1) };
        let Some((group, scheme, path)) = snapshot.get(index).cloned() else { return };
        self.history.navigating_recently_used = true;
        if self.groups_in_grid_order().contains(&group) {
            self.focus_group(group, window, cx);
        }
        if let Some(found) = (0..self.editors.editors.len()).find(|&found| self.editors.editors[found].document().is(&scheme, &path)) {
            self.editors.activate(found);
            self.editors_changed(cx);
        }
        self.history.navigating_recently_used = false;
        self.history.recently_used = Some((snapshot, index));
    }

    fn editors_by_recency(&self, in_group: bool) -> Vec<(u32, String, PathBuf)> {
        let groups = if in_group { vec![self.editors.id] } else { self.editors.group_ids_mru() };
        let mut editors = Vec::new();
        for group in groups {
            for editor in self.editors.group_editors_by_recency(group) {
                let document = editor.document();
                editors.push((group, document.scheme.clone(), document.path.clone()));
            }
        }
        editors
    }
}
