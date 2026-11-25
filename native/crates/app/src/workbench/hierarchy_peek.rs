//! callHierarchy.contribution.ts with callHierarchyPeek.ts, and typeHierarchy.contribution.ts with
//! typeHierarchyPeek.ts: the peek of the calls to or from the symbol at the cursor, or of its super- and subtypes,
//! the item's file previewed on the left; and the `_executePrepareCallHierarchy`-family API commands.

use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, MouseButton, MouseDownEvent, ScrollWheelEvent, Stateful, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::peek_view::{LayoutStorage, PeekKind, PeekLayout};
use crate::commands::Outcome;
use crate::editor::{LINE_HEIGHT, OpenEditor, Position, TextDocument, VerticalReveal, utf16_column};
use crate::hierarchy_model::{Direction, HierarchyItem, HierarchyKind, HierarchyTree};
use crate::language_features::Candidate;

/// callHierarchyTree.ts `VirtualDelegate.getHeight`.
const ROW_HEIGHT: f32 = 22.;
/// `_models` of callHierarchy.ts keeps the 10 newest sessions for the API commands.
const API_SESSIONS: usize = 10;

struct Spec {
    register: &'static str,
    prepare: &'static str,
    release: &'static str,
    layout: LayoutStorage,
    direction_key: &'static str,
    failed: &'static str,
}

const CALLS: Spec = Spec {
    register: "$registerCallHierarchyProvider",
    prepare: "$prepareCallHierarchy",
    release: "$releaseCallHierarchy",
    layout: LayoutStorage { key: "callHierarchyPeekLayout", height_key: "height", default_height: 17. },
    direction_key: "callHierarchy/defaultDirection",
    failed: "Failed to show call hierarchy",
};

const TYPES: Spec = Spec {
    register: "$registerTypeHierarchyProvider",
    prepare: "$prepareTypeHierarchy",
    release: "$releaseTypeHierarchy",
    layout: LayoutStorage { key: "typeHierarchyPeekLayout", height_key: "height", default_height: 17. },
    direction_key: "typeHierarchy/defaultDirection",
    failed: "Failed to show type hierarchy",
};

fn spec(kind: HierarchyKind) -> &'static Spec {
    match kind {
        HierarchyKind::Calls => &CALLS,
        HierarchyKind::Types => &TYPES,
    }
}

pub(super) struct HierarchyPeek {
    widget: Option<HierarchyWidget>,
    tree_focus: FocusHandle,
    /// The sessions the API commands prepared: kind, provider handle and session id, the newest last.
    api_sessions: Vec<(HierarchyKind, u64, String)>,
}

impl HierarchyPeek {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Self {
        HierarchyPeek { widget: None, tree_focus: cx.focus_handle(), api_sessions: Vec::new() }
    }
}

enum State {
    Loading,
    Message(String),
    Data,
}

struct HierarchyWidget {
    layout: PeekLayout,
    kind: HierarchyKind,
    handle: u64,
    direction: Direction,
    state: State,
    roots: Vec<HierarchyItem>,
    tree: Option<HierarchyTree>,
    /// The tree's focus (the keys move it and the preview follows) and its selection (a click).
    focused: Option<usize>,
    selected: Option<usize>,
    tree_scroll: f32,
    preview_scroll: f32,
    documents: Vec<TextDocument>,
    /// The prepare and children requests of this peek, dropped with it.
    _requests: Vec<Task<()>>,
}

impl Workbench {
    pub(super) fn register_hierarchy_commands(&mut self) {
        for (kind, show, close, refocus, flips) in [
            (HierarchyKind::Calls, "editor.showCallHierarchy", "editor.closeCallHierarchy", "editor.refocusCallHierarchy", [("editor.showIncomingCalls", Direction::CallsTo), ("editor.showOutgoingCalls", Direction::CallsFrom)]),
            (HierarchyKind::Types, "editor.showTypeHierarchy", "editor.closeTypeHierarchy", "editor.refocusTypeHierarchy", [("editor.showSupertypes", Direction::Supertypes), ("editor.showSubtypes", Direction::Subtypes)]),
        ] {
            self.commands.register(show, move |workbench, _, window, cx| {
                workbench.start_hierarchy(kind, window, cx);
                Outcome::done()
            });
            self.commands.register(close, move |workbench, _, window, cx| {
                if workbench.hierarchy_peek.widget.as_ref().is_some_and(|widget| widget.kind == kind) {
                    workbench.close_hierarchy_peek(true, window, cx);
                }
                Outcome::done()
            });
            self.commands.register(refocus, move |workbench, _, window, cx| {
                workbench.refocus_hierarchy(kind, window, cx);
                Outcome::done()
            });
            for (id, direction) in flips {
                self.commands.register(id, move |workbench, _, window, cx| {
                    workbench.set_hierarchy_direction(kind, direction, window, cx);
                    Outcome::done()
                });
            }
        }
        for (id, kind) in [("_executePrepareCallHierarchy", HierarchyKind::Calls), ("_executePrepareTypeHierarchy", HierarchyKind::Types)] {
            self.commands.register(id, move |workbench, args, _, cx| workbench.api_prepare_hierarchy(kind, &args, cx));
        }
        for (id, kind, direction) in [
            ("_executeProvideIncomingCalls", HierarchyKind::Calls, Direction::CallsTo),
            ("_executeProvideOutgoingCalls", HierarchyKind::Calls, Direction::CallsFrom),
            ("_executeProvideSupertypes", HierarchyKind::Types, Direction::Supertypes),
            ("_executeProvideSubtypes", HierarchyKind::Types, Direction::Subtypes),
        ] {
            self.commands.register(id, move |workbench, args, _, cx| workbench.api_resolve_hierarchy(kind, direction, &args, cx));
        }
    }

    pub(super) fn hierarchy_peek_layout(&self) -> Option<&PeekLayout> {
        self.hierarchy_peek.widget.as_ref().map(|widget| &widget.layout)
    }

    pub(super) fn hierarchy_peek_layout_mut(&mut self) -> Option<&mut PeekLayout> {
        self.hierarchy_peek.widget.as_mut().map(|widget| &mut widget.layout)
    }

    /// The first provider of `kind` for the active editor's model, by score (`ProviderRegistry.ordered(model)[0]`).
    fn hierarchy_provider(&self, kind: HierarchyKind, editor: &OpenEditor) -> Option<u64> {
        let uri = editor.document().uri();
        self.language_features.ordered(spec(kind).register, &Candidate { uri: &uri, language: &editor.document().language_id }).first().map(|registration| registration.handle)
    }

    /// `editorHasCallHierarchyProvider`, `callHierarchyVisible` and `callHierarchyDirection`, and the type ones.
    pub(super) fn hierarchy_peek_context(&self, editor: &OpenEditor, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("editorHasCallHierarchyProvider".into(), json!(self.hierarchy_provider(HierarchyKind::Calls, editor).is_some()));
        context.insert("editorHasTypeHierarchyProvider".into(), json!(self.hierarchy_provider(HierarchyKind::Types, editor).is_some()));
        let Some(widget) = &self.hierarchy_peek.widget else { return };
        let (visible, direction) = match widget.kind {
            HierarchyKind::Calls => ("callHierarchyVisible", "callHierarchyDirection"),
            HierarchyKind::Types => ("typeHierarchyVisible", "typeHierarchyDirection"),
        };
        context.insert(visible.into(), json!(true));
        context.insert(direction.into(), json!(widget.direction.storage_value()));
        if self.hierarchy_peek.tree_focus.is_focused(window) {
            context.insert("listFocus".into(), json!(true));
        }
    }

    /// `startCallHierarchyFromEditor`: the peek opens at the cursor in the stored direction, loading until the
    /// provider prepared the items.
    fn start_hierarchy(&mut self, kind: HierarchyKind, window: &mut Window, cx: &mut Context<Self>) {
        self.close_hierarchy_peek(false, window, cx);
        let Some(editor) = self.editors.active() else { return };
        let Some(handle) = self.hierarchy_provider(kind, editor) else { return };
        let position = editor.selection().head;
        let stored = self.profile_storage.lock().unwrap().get(spec(kind).direction_key).map(String::from);
        let direction = Direction::from_storage(kind, stored.as_deref());
        self.open_hierarchy_peek(kind, handle, position, direction, cx);
        let editor = self.editors.active().expect("checked above");
        let position_dto = json!({ "lineNumber": position.line + 1, "column": utf16_column(&editor.document().lines[position.line], position.column) + 1 });
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", spec(kind).prepare, vec![json!(handle), editor.document().uri(), position_dto]);
        let request = cx.spawn_in(window, async move |workbench, cx| {
            let answer = call.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.hierarchy_prepared(answer.ok(), window, cx));
        });
        self.hierarchy_peek.widget.as_mut().expect("just opened")._requests.push(request);
    }

    fn open_hierarchy_peek(&mut self, kind: HierarchyKind, handle: u64, position: Position, direction: Direction, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        self.editors.active_mut().expect("the peek opens in the active editor").reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport);
        let layout = self.open_peek_zone(&spec(kind).layout, position);
        self.hierarchy_peek.widget = Some(HierarchyWidget {
            layout,
            kind,
            handle,
            direction,
            state: State::Loading,
            roots: Vec::new(),
            tree: None,
            focused: None,
            selected: None,
            tree_scroll: 0.,
            preview_scroll: 0.,
            documents: Vec::new(),
            _requests: Vec::new(),
        });
        self.editor_scrolled(cx);
        cx.notify();
    }

    /// The provider prepared the items: "No results" without any, else the tree of the first one.
    fn hierarchy_prepared(&mut self, answer: Option<Result<Value, Value>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.hierarchy_peek.widget.as_mut() else { return };
        let roots: Vec<HierarchyItem> = match answer {
            Some(Ok(items)) => items.as_array().into_iter().flatten().filter_map(HierarchyItem::from_dto).collect(),
            _ => {
                widget.state = State::Message(spec(widget.kind).failed.into());
                return cx.notify();
            }
        };
        if roots.is_empty() {
            widget.state = State::Message("No results".into());
            return cx.notify();
        }
        // `CallHierarchyModel.create`: the session's first root.
        widget.roots = roots.into_iter().take(1).collect();
        self.show_hierarchy_model(window, cx);
    }

    /// `showModel`: the root expanded; without children the empty message, else its first child focused and
    /// previewed and the tree focused.
    fn show_hierarchy_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let widget = self.hierarchy_peek.widget.as_mut().expect("shown with a widget");
        widget.tree = Some(HierarchyTree::new(widget.direction, widget.roots.clone()));
        (widget.focused, widget.selected) = (None, None);
        widget.tree_scroll = 0.;
        self.expand_hierarchy_node(0, window, cx);
    }

    /// `DataSource.getChildren` for a node not asked yet, then the node expanded; the root's answer decides what
    /// the peek shows.
    fn expand_hierarchy_node(&mut self, node: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.hierarchy_peek.widget.as_mut() else { return };
        let Some(tree) = widget.tree.as_mut() else { return };
        if tree.nodes[node].children.is_some() {
            tree.nodes[node].expanded = true;
            return cx.notify();
        }
        let item = &tree.nodes[node].item;
        let args = vec![json!(widget.handle), json!(item.session_id()), json!(item.item_id())];
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", tree.direction.method(), args);
        let direction = tree.direction;
        let request = cx.spawn_in(window, async move |workbench, cx| {
            let answer = call.await.ok().and_then(Result::ok).unwrap_or(Value::Null);
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.hierarchy_children_answered(direction, node, &answer, window, cx));
        });
        widget._requests.push(request);
    }

    fn hierarchy_children_answered(&mut self, direction: Direction, node: usize, answer: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.hierarchy_peek.widget.as_mut() else { return };
        let Some(tree) = widget.tree.as_mut().filter(|tree| tree.direction == direction) else { return };
        tree.set_children(node, answer);
        tree.nodes[node].expanded = true;
        if node == 0 {
            let children = tree.nodes[0].children.clone().unwrap_or_default();
            match children.first() {
                None => widget.state = State::Message(direction.empty_message(&tree.root().name)),
                Some(&first) => {
                    widget.state = State::Data;
                    widget.focused = Some(first);
                    self.update_hierarchy_preview();
                    window.focus(&self.hierarchy_peek.tree_focus, cx);
                }
            }
        }
        cx.notify();
    }

    /// `_updatePreview`: the focused row's preview file read and centered on the marked ranges.
    fn update_hierarchy_preview(&mut self) {
        let Some(widget) = self.hierarchy_peek.widget.as_ref() else { return };
        let (Some(tree), Some(focused)) = (&widget.tree, widget.focused) else { return };
        let (uri, resource, ranges) = tree.preview(focused);
        let mut documents = std::mem::take(&mut self.hierarchy_peek.widget.as_mut().expect("checked above").documents);
        self.read_peek_document(&uri, &resource, &mut documents);
        let line_count = self.peek_document(&uri, &documents).map_or(1, |document| document.lines.len());
        let line = match ranges.iter().min() {
            Some(first) => (first.start.0 + ranges.iter().map(|range| range.end.0).max().unwrap_or(first.start.0)) / 2,
            None => 1,
        };
        let scroll = self.peek_preview_scroll(line.saturating_sub(1), line_count, self.peek_body_height(PeekKind::Hierarchy));
        let widget = self.hierarchy_peek.widget.as_mut().expect("checked above");
        widget.documents = documents;
        widget.preview_scroll = scroll;
    }

    /// `endCallHierarchy` on the peek's close: the layout and the direction stored, the session released.
    pub(super) fn close_hierarchy_peek(&mut self, focus_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.hierarchy_peek.widget.take() else { return };
        let spec = spec(widget.kind);
        self.close_peek_zone(&widget.layout, Some(&spec.layout));
        self.profile_storage.lock().unwrap().set(spec.direction_key, widget.direction.storage_value().to_string());
        for root in &widget.roots {
            self.host.send("ExtHostLanguageFeatures", spec.release, vec![json!(widget.handle), json!(root.session_id())]);
        }
        if focus_editor {
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// The controller's `onDidChangeModel`, run on every render: another model in the editor ends the session.
    pub(super) fn follow_hierarchy_peek_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = &self.hierarchy_peek.widget else { return };
        if self.editors.active().is_none_or(|editor| editor.model_id != widget.layout.model_id) {
            self.close_hierarchy_peek(false, window, cx);
        }
    }

    /// `updateDirection`: the tree asked again from the root in the other direction.
    fn set_hierarchy_direction(&mut self, kind: HierarchyKind, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.hierarchy_peek.widget.as_mut().filter(|widget| widget.kind == kind && widget.direction != direction) else { return };
        widget.direction = direction;
        if widget.roots.is_empty() {
            return cx.notify();
        }
        self.show_hierarchy_model(window, cx);
    }

    /// `startCallHierarchyFromCallHierarchy`: the focused item becomes the root of a new peek in its file.
    fn refocus_hierarchy(&mut self, kind: HierarchyKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(widget) = self.hierarchy_peek.widget.as_ref().filter(|widget| widget.kind == kind) else { return };
        let (Some(tree), Some(focused)) = (&widget.tree, widget.focused) else { return };
        let item = tree.nodes[focused].item.clone();
        let (handle, direction) = (widget.handle, widget.direction);
        // The fork shares the session; the old peek closes without releasing it.
        let mut widget = self.hierarchy_peek.widget.take().expect("checked above");
        widget.roots.clear();
        self.hierarchy_peek.widget = Some(widget);
        self.close_hierarchy_peek(false, window, cx);
        let start = item.selection_range.start;
        let selection = json!({ "startLineNumber": start.0, "startColumn": start.1, "endLineNumber": start.0, "endColumn": start.1 });
        if self.try_show_text_document(&item.resource, &json!({ "selection": selection }), window, cx).is_err() {
            return;
        }
        let position = self.editors.active().expect("just shown").selection().head;
        self.open_hierarchy_peek(kind, handle, position, direction, cx);
        self.hierarchy_peek.widget.as_mut().expect("just opened").roots = vec![item];
        self.show_hierarchy_model(window, cx);
    }

    /// A double click or Enter on a row: the peek closes and the item opens pinned at its name.
    fn open_hierarchy_item(&mut self, node: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.hierarchy_peek.widget.as_ref().and_then(|widget| widget.tree.as_ref()) else { return };
        let item = tree.nodes[node].item.clone();
        self.close_hierarchy_peek(true, window, cx);
        let range = item.selection_range;
        let selection = json!({ "startLineNumber": range.start.0, "startColumn": range.start.1, "endLineNumber": range.end.0, "endColumn": range.end.1 });
        let _ = self.try_show_text_document(&item.resource, &json!({ "selection": selection, "pinned": true }), window, cx);
    }

    fn hierarchy_rows(&self) -> Vec<usize> {
        self.hierarchy_peek.widget.as_ref().and_then(|widget| widget.tree.as_ref()).map(HierarchyTree::rows).unwrap_or_default()
    }

    fn focus_hierarchy_row(&mut self, node: usize, cx: &mut Context<Self>) {
        let rows = self.hierarchy_rows();
        let body_height = self.peek_body_height(PeekKind::Hierarchy);
        let Some(widget) = self.hierarchy_peek.widget.as_mut() else { return };
        widget.focused = Some(node);
        if let Some(index) = rows.iter().position(|row| *row == node) {
            let top = index as f32 * ROW_HEIGHT;
            widget.tree_scroll = widget.tree_scroll.min(top).max(top + ROW_HEIGHT - body_height);
        }
        self.update_hierarchy_preview();
        cx.notify();
    }

    /// The tree's list keys while it has the focus.
    pub(super) fn hierarchy_peek_list_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.hierarchy_peek.widget.is_none() || !self.hierarchy_peek.tree_focus.is_focused(window) {
            return false;
        }
        let rows = self.hierarchy_rows();
        let widget = self.hierarchy_peek.widget.as_ref().expect("checked above");
        let index = widget.focused.and_then(|focused| rows.iter().position(|row| *row == focused));
        let last = rows.len().saturating_sub(1);
        let target = match id {
            "list.focusDown" => index.map_or(0, |index| (index + 1).min(last)),
            "list.focusUp" => index.map_or(0, |index| index.saturating_sub(1)),
            "list.focusFirst" => 0,
            "list.focusLast" => last,
            "list.expand" | "list.collapse" | "list.toggleExpand" | "list.select" => {
                let (Some(node), Some(tree)) = (widget.focused, widget.tree.as_ref()) else { return true };
                let expanded = tree.nodes[node].expanded;
                let parent = tree.nodes[node].parent;
                match (id, expanded) {
                    ("list.select", _) => self.open_hierarchy_item(node, window, cx),
                    ("list.expand", false) | ("list.toggleExpand", false) => self.expand_hierarchy_node(node, window, cx),
                    ("list.collapse", true) | ("list.toggleExpand", true) => self.collapse_hierarchy_node(node, cx),
                    ("list.collapse", false) => {
                        if let Some(parent) = parent {
                            self.focus_hierarchy_row(parent, cx);
                        }
                    }
                    _ => {}
                }
                return true;
            }
            _ => return false,
        };
        if let Some(&node) = rows.get(target) {
            self.focus_hierarchy_row(node, cx);
        }
        true
    }

    fn collapse_hierarchy_node(&mut self, node: usize, cx: &mut Context<Self>) {
        if let Some(tree) = self.hierarchy_peek.widget.as_mut().and_then(|widget| widget.tree.as_mut()) {
            tree.nodes[node].expanded = false;
        }
        cx.notify();
    }

    /// The peek: the title with the direction toggle and close in the head, the preview and the tree in the body.
    pub(super) fn hierarchy_peek_widget(&self, editor: &OpenEditor, content_left: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let widget = self.hierarchy_peek.widget.as_ref().filter(|widget| widget.layout.model_id == editor.model_id)?;
        let label = match (&widget.state, &widget.tree) {
            (State::Loading, _) => "Loading...".to_string(),
            (State::Data, Some(tree)) => widget.direction.title(&tree.root().name),
            _ => String::new(),
        };
        let title = self.peek_title_parts(div().id("hierarchy-peek-title"), label, String::new(), String::new());
        let (flip, close) = match widget.direction {
            Direction::CallsTo => (("call-outgoing", "Show Outgoing Calls", "editor.showOutgoingCalls"), "editor.closeCallHierarchy"),
            Direction::CallsFrom => (("call-incoming", "Show Incoming Calls", "editor.showIncomingCalls"), "editor.closeCallHierarchy"),
            Direction::Subtypes => (("type-hierarchy-super", "Show Supertypes", "editor.showSupertypes"), "editor.closeTypeHierarchy"),
            Direction::Supertypes => (("type-hierarchy-sub", "Show Subtypes", "editor.showSubtypes"), "editor.closeTypeHierarchy"),
        };
        let actions = vec![self.peek_action(flip.0, flip.1, flip.2, cx).into_any_element(), self.peek_action("close", "Close", close, cx).into_any_element()];
        let head = self.peek_head(title, actions);
        self.peek_zone(PeekKind::Hierarchy, editor, content_left, char_width, head, |preview_width, body_height, cx| match &widget.state {
            State::Loading => (div().into_any_element(), false),
            State::Message(message) => (self.peek_message(message.clone()), false),
            State::Data => {
                let preview = self.hierarchy_preview(widget, preview_width, body_height, char_width, window, cx);
                (div().size_full().flex().child(preview).child(self.hierarchy_tree(widget, body_height, window, cx)).into_any_element(), true)
            }
        }, cx)
    }

    fn hierarchy_preview(&self, widget: &HierarchyWidget, width: f32, height: f32, char_width: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let preview = widget.tree.as_ref().zip(widget.focused).map(|(tree, focused)| tree.preview(focused));
        let document = preview.as_ref().and_then(|(uri, ..)| self.peek_document(uri, &widget.documents));
        let marks: Vec<(usize, usize, usize, usize)> = preview.iter().flat_map(|(_, _, ranges)| ranges).map(|range| (range.start.0 - 1, range.start.1 - 1, range.end.0 - 1, range.end.1 - 1)).collect();
        // The preview only reveals the marks (`revealRangeInCenter`); its cursor stays on the first line.
        let cursor_line = 0;
        let line_count = document.map_or(0, |document| document.lines.len());
        self.peek_preview_lines("hierarchy-peek-preview", document.as_deref(), &marks, cursor_line, widget.preview_scroll, width, height, char_width, window)
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y);
                let max = (line_count as f32 * LINE_HEIGHT - height).max(0.);
                let Some(widget) = workbench.hierarchy_peek.widget.as_mut() else { return };
                widget.preview_scroll = (widget.preview_scroll - delta).clamp(0., max);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                // `_editor.onMouseDown` with a double click: the focused item's file opens.
                if event.click_count == 2
                    && let Some(node) = workbench.hierarchy_peek.widget.as_ref().and_then(|widget| widget.focused)
                {
                    workbench.open_hierarchy_item(node, window, cx);
                }
            }))
    }

    fn hierarchy_tree(&self, widget: &HierarchyWidget, height: f32, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.hierarchy_peek.tree_focus.is_focused(window);
        let rows = self.hierarchy_rows();
        let first = (widget.tree_scroll / ROW_HEIGHT).floor() as usize;
        let last = (((widget.tree_scroll + height) / ROW_HEIGHT).ceil() as usize).min(rows.len());
        let elements: Vec<AnyElement> = (first..last).map(|index| self.hierarchy_row(widget, rows[index], index, focused, cx)).collect();
        let count = rows.len();
        div()
            .id("hierarchy-peek-tree")
            .track_focus(&self.hierarchy_peek.tree_focus)
            .flex_1()
            .min_w_0()
            .h(px(height))
            .relative()
            .overflow_hidden()
            .font_family(super::peek_view::UI_FONT)
            .text_size(px(13.))
            .line_height(px(ROW_HEIGHT))
            .bg(self.color("peekViewResult.background"))
            .text_color(self.color("peekViewResult.lineForeground"))
            .children(elements)
            .on_scroll_wheel(cx.listener(move |workbench, event: &ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(ROW_HEIGHT)).y);
                let Some(widget) = workbench.hierarchy_peek.widget.as_mut() else { return };
                widget.tree_scroll = (widget.tree_scroll - delta).clamp(0., (count as f32 * ROW_HEIGHT - height).max(0.));
                cx.stop_propagation();
                cx.notify();
            }))
    }

    /// `CallRenderer` / `TypeRenderer`: the twistie, the colored symbol icon, the name and the detail.
    fn hierarchy_row(&self, widget: &HierarchyWidget, node: usize, index: usize, tree_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let tree = widget.tree.as_ref().expect("rows of a tree");
        let entry = &tree.nodes[node];
        let item = &entry.item;
        let is_focused = widget.focused == Some(node);
        let is_selected = widget.selected == Some(node);
        // AsyncDataTree: every row may have children until asked; an empty answer drops the twistie.
        let twistie = match (&entry.children, entry.expanded) {
            (Some(children), _) if children.is_empty() => None,
            (_, true) => Some("chevron-down"),
            (_, false) => Some("chevron-right"),
        };
        let twistie = match twistie {
            Some(icon) => self.icon(icon, 16., 16., self.foreground_or("icon.foreground")).id(("hierarchy-twistie", index)).cursor_pointer().on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let expanded = workbench.hierarchy_peek.widget.as_ref().and_then(|widget| widget.tree.as_ref()).is_some_and(|tree| tree.nodes[node].expanded);
                match expanded {
                    true => workbench.collapse_hierarchy_node(node, cx),
                    false => workbench.expand_hierarchy_node(node, window, cx),
                }
            })),
            None => div().id(("hierarchy-twistie", index)).size(px(16.)).flex_none(),
        };
        let codicon = crate::symbol_icons::codicon(item.kind);
        let icon_color = match tree_focused && is_selected {
            true => self.list_selection_icon_color(),
            false => crate::symbol_icons::color_key(codicon).and_then(|key| self.theme.color(key)).unwrap_or(self.color("foreground")),
        };
        let selection = match tree_focused {
            true => self.theme.color("peekViewResult.selectionBackground"),
            false => self.theme.color("list.inactiveSelectionBackground"),
        };
        let focus_outline = match is_selected {
            true => self.theme.color("list.focusAndSelectionOutline").or(self.theme.color("contrastActiveBorder")),
            false => self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")),
        };
        let focus_outline = focus_outline.filter(|_| tree_focused && is_focused);
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(self.icon(codicon, 16., 16., icon_color).mr(px(6.)).flex_none())
            .child(div().flex_none().when(item.deprecated, |name| name.line_through()).child(item.name.replace('\n', "\u{23ce}")))
            .when(!item.detail.is_empty(), |label| label.child(div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).min_w_0().overflow_hidden().text_ellipsis().child(item.detail.clone())));
        div()
            .id(("hierarchy-row", index))
            .absolute()
            .left_0()
            .right_0()
            .top(px(index as f32 * ROW_HEIGHT - widget.tree_scroll))
            .h(px(ROW_HEIGHT))
            .cursor_pointer()
            .flex()
            .items_center()
            .pl(px(self.tree_indent() * entry.depth as f32 + 4.))
            .when(!is_selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .when(is_selected, |row| row.when_some(selection, |row, color| row.bg(color)).when(tree_focused, |row| row.text_color(self.color("peekViewResult.selectionForeground"))))
            .when_some(focus_outline, |row, color| row.child(div().absolute().inset_0().border_1().border_color(color)))
            .child(twistie)
            .child(label.ml(px(6.)))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                window.focus(&workbench.hierarchy_peek.tree_focus, cx);
                match event.click_count() {
                    2 => workbench.open_hierarchy_item(node, window, cx),
                    _ => {
                        if let Some(widget) = workbench.hierarchy_peek.widget.as_mut() {
                            widget.selected = Some(node);
                        }
                        workbench.focus_hierarchy_row(node, cx);
                    }
                }
            }))
            .into_any_element()
    }

    /// `_executePrepareCallHierarchy` / `_executePrepareTypeHierarchy`: the first provider's roots of the model at
    /// the position, the session kept for the resolve commands.
    fn api_prepare_hierarchy(&mut self, kind: HierarchyKind, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let Some((scheme, path)) = args.first().and_then(|uri| super::host_requests::uri_parts(uri).ok()) else { return Outcome::error("Illegal argument") };
        let Some(editor) = self.editors.editors.iter().chain(&self.editors.models).find(|editor| editor.document().is(scheme, &path)) else { return Outcome::error("Illegal argument") };
        let Some(handle) = self.hierarchy_provider(kind, editor) else { return Outcome::Ready(Ok(json!([]))) };
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", spec(kind).prepare, vec![json!(handle), editor.document().uri(), args.get(1).cloned().unwrap_or_default()]);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let roots = match call.await {
                Ok(Ok(Value::Array(items))) if !items.is_empty() => items,
                _ => return drop(sender.send(Ok(json!([])))),
            };
            let session = roots[0]["_sessionId"].as_str().unwrap_or_default().to_string();
            let _ = workbench.update(cx, |workbench, _| {
                let sessions = &mut workbench.hierarchy_peek.api_sessions;
                sessions.push((kind, handle, session));
                if sessions.len() > API_SESSIONS {
                    let (kind, handle, session) = sessions.remove(0);
                    workbench.host.send("ExtHostLanguageFeatures", spec(kind).release, vec![json!(handle), json!(session)]);
                }
            });
            let _ = sender.send(Ok(json!([roots[0]])));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `_executeProvideIncomingCalls` and the others: the item's session's provider asked, `[]` for an unknown one.
    fn api_resolve_hierarchy(&mut self, kind: HierarchyKind, direction: Direction, args: &[Value], cx: &mut Context<Self>) -> Outcome {
        let item = args.first().cloned().unwrap_or_default();
        let session = item["_sessionId"].as_str().unwrap_or_default();
        let Some(&(_, handle, _)) = self.hierarchy_peek.api_sessions.iter().find(|(known, _, id)| *known == kind && id == session) else { return Outcome::Ready(Ok(json!([]))) };
        let call = self.host.call_cancellable("ExtHostLanguageFeatures", direction.method(), vec![json!(handle), json!(session), item["_itemId"].clone()]);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |_, _| {
            let answer = match call.await {
                Ok(Ok(Value::Array(items))) => Value::Array(items),
                _ => json!([]),
            };
            let _ = sender.send(Ok(answer));
        })
        .detach();
        Outcome::Pending(receiver)
    }
}

#[cfg(test)]
mod tests;
