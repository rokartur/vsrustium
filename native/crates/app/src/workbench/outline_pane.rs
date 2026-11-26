//! outlinePane.ts with documentSymbolsOutline.ts and documentSymbolsTree.ts: the Explorer's Outline view, the
//! document symbols of the active editor as a tree, in position order; a symbol opens at its name.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, KeyDownEvent, Task, Window, div, prelude::*, px};

use super::Workbench;
use super::outline_actions::SortOrder;
use super::quick_access::goto_symbol::position_of;
use crate::editor::{Selection, VerticalReveal};
use crate::outline_model::{OutlineChildren, OutlineElement, OutlineMarker, OutlineModel, Range};

/// documentSymbolsTree.ts `DocumentSymbolVirtualDelegate.getHeight`.
const ROW_HEIGHT: f32 = 22.;
/// MarkerSeverity.Error.
const ERROR: u32 = 8;
/// SymbolTag.Deprecated.
const DEPRECATED: u64 = 1;
/// `DocumentSymbolFilter.kindToConfigName`, by `SymbolKind`.
const SHOW_KIND: [&str; 26] = [
    "showFiles", "showModules", "showNamespaces", "showPackages", "showClasses", "showMethods", "showProperties", "showFields", "showConstructors",
    "showEnums", "showInterfaces", "showFunctions", "showVariables", "showConstants", "showStrings", "showNumbers", "showBooleans", "showArrays",
    "showObjects", "showKeys", "showNull", "showEnumMembers", "showStructs", "showEvents", "showOperators", "showTypeParameters",
];

pub(super) struct OutlinePane {
    pub(super) focus: FocusHandle,
    /// The active editor's model and version the outline was asked for.
    key: Option<(u32, u32)>,
    pub(super) status: Status,
    /// The rows whose expansion differs from `outline.collapseItems`, per model.
    toggled: HashSet<(u32, String)>,
    selected: Option<String>,
    pub(super) view_state: super::outline_actions::OutlineViewState,
    /// The model and cursor `revealActiveElement` last followed.
    followed: Option<(u32, crate::editor::Position)>,
    _request: Option<Task<()>>,
    _loading: Option<Task<()>>,
}

pub(super) enum Status {
    NoProvider,
    /// `Loading document symbols for '{0}'...`, shown 100ms after the request.
    Loading(String),
    Ready { model_id: u32, name: String, outline: OutlineModel },
}

impl OutlinePane {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Self {
        OutlinePane { focus: cx.focus_handle(), key: None, status: Status::NoProvider, toggled: HashSet::new(), selected: None, view_state: Default::default(), followed: None, _request: None, _loading: None }
    }
}

/// A visible row: its depth, its id, and the group label or the element.
pub(super) struct Row<'a> {
    pub(super) depth: usize,
    pub(super) id: &'a str,
    group: Option<&'a str>,
    element: Option<&'a OutlineElement>,
    pub(super) has_children: bool,
}

impl Workbench {
    /// outlinePane.ts listens to `IOutlineService.onDidChange`: a provider registered after the editor opened
    /// builds the outline again.
    pub(super) fn outline_providers_changed(&mut self, cx: &mut Context<Self>) {
        self.outline_pane.key = None;
        self.refresh_outline_pane(cx);
    }

    /// `_handleEditorControlChanged` and `_createOutline`: a new model or version of the active editor asks the
    /// outline again; the previous tree stays until the answer, the loading message shows after 100ms.
    pub(super) fn refresh_outline_pane(&mut self, cx: &mut Context<Self>) {
        self.reveal_outline_active_element(false, cx);
        let key = self.editors.active().map(|editor| (editor.model_id, editor.text_model().version_id));
        if self.outline_pane.key == key {
            return;
        }
        self.outline_pane.key = key;
        self.outline_pane._loading = None;
        let Some((model_id, _)) = key.filter(|(model_id, _)| self.has_document_symbol_provider(*model_id)) else {
            self.outline_pane.status = Status::NoProvider;
            self.outline_pane._request = None;
            return;
        };
        let name = self.editors.active().expect("checked above").document().name();
        if !matches!(&self.outline_pane.status, Status::Ready { model_id: shown, .. } if *shown == model_id) {
            let name = name.clone();
            self.outline_pane._loading = Some(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    workbench.outline_pane.status = Status::Loading(name);
                    cx.notify();
                });
            }));
        }
        let outline = self.outline_model(model_id, cx);
        self.outline_pane._request = Some(cx.spawn(async move |workbench, cx| {
            let Some(outline) = outline.await else { return };
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.outline_pane._loading = None;
                workbench.outline_pane.status = Status::Ready { model_id, name, outline };
                workbench.reveal_outline_active_element(true, cx);
                cx.notify();
            });
        }));
    }

    /// `.outline-pane`: the message, or the tree of the active editor's symbols.
    pub(super) fn outline_pane_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(body) = self.notebook_outline_pane_body(window, cx) {
            return body;
        }
        let message = |text: String| div().pt(px(10.)).px(px(22.)).opacity(0.5).line_height(px(18.2)).child(text).into_any_element();
        let (model_id, outline) = match &self.outline_pane.status {
            Status::NoProvider => return message("The active editor cannot provide outline information.".into()),
            Status::Loading(name) => return message(format!("Loading document symbols for '{name}'...")),
            Status::Ready { name, outline, .. } if outline.groups.is_empty() => return message(format!("No symbols found in document '{name}'")),
            Status::Ready { model_id, outline, .. } => (*model_id, self.outline_with_markers(*model_id, outline)),
        };
        let focused = self.outline_pane.focus.is_focused(window);
        let rows = self.outline_rows(model_id, &outline);
        let elements: Vec<AnyElement> = rows.iter().enumerate().map(|(index, row)| self.outline_row(model_id, row, index, focused, cx)).collect();
        div()
            .id("outline-tree")
            .track_focus(&self.outline_pane.focus)
            .size_full()
            .overflow_y_scroll()
            .line_height(px(ROW_HEIGHT))
            .children(elements)
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| {
                if workbench.outline_key_down(event.keystroke.key.as_str(), window, cx) {
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }

    /// `_applyMarkersToOutline`: the errors and warnings of the model counted per element, with `problems.visibility`
    /// and `outline.problems.enabled`.
    fn outline_with_markers(&self, model_id: u32, outline: &OutlineModel) -> OutlineModel {
        let mut outline = outline.clone();
        let editor = self.editors.editors.iter().chain(&self.editors.models).find(|editor| editor.model_id == model_id);
        let (Some(editor), true, true) = (editor, self.setting("problems.visibility") != false, self.setting("outline.problems.enabled") != false) else { return outline };
        let markers: Vec<OutlineMarker> = editor
            .text_model()
            .markers
            .iter()
            .filter(|decoration| decoration.marker.severity >= 4)
            .map(|decoration| {
                let marker = &decoration.marker;
                OutlineMarker { range: Range::new(marker.start_line, marker.start_column, marker.end_line, marker.end_column), severity: marker.severity as u32 }
            })
            .collect();
        outline.update_markers(&markers);
        outline
    }

    /// The rows the tree shows: the groups of several providers, then each element below its expanded parent,
    /// siblings in the Sort By order (`OutlineTreeSorter`).
    pub(super) fn outline_rows<'a>(&self, model_id: u32, outline: &'a OutlineModel) -> Vec<Row<'a>> {
        let expanded = |id: &str| self.outline_row_expanded(model_id, id);
        let shown = |element: &OutlineElement| self.shows_symbol_kind("outline", element.symbol.kind);
        let sort_by = self.outline_pane.view_state.sort_by;
        let mut rows = Vec::new();
        let push_elements = |rows: &mut Vec<Row<'a>>, elements, depth| push_elements(rows, elements, depth, sort_by, &expanded, &shown);
        match outline.children {
            OutlineChildren::Elements => push_elements(&mut rows, &outline.groups[0].children, 0),
            OutlineChildren::Groups => {
                for group in &outline.groups {
                    rows.push(Row { depth: 0, id: &group.id, group: Some(&group.label), element: None, has_children: true });
                    if expanded(&group.id) {
                        push_elements(&mut rows, &group.children, 1);
                    }
                }
            }
        }
        rows
    }

    /// `DocumentSymbolRenderer`: twistie, colored symbol icon, name and detail, and the problem badge.
    fn outline_row(&self, model_id: u32, row: &Row, index: usize, tree_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id.to_string();
        let selected = self.outline_pane.selected.as_deref() == Some(row.id);
        let indent = self.tree_indent();
        let twistie = match row.has_children {
            true => {
                let expanded = self.outline_row_expanded(model_id, &id);
                let chevron = if expanded { "chevron-down" } else { "chevron-right" };
                let toggled = id.clone();
                self.icon(chevron, 16., 16., self.foreground_or("icon.foreground")).id(("outline-twistie", index)).cursor_pointer().on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    workbench.toggle_outline_row(model_id, &toggled, cx);
                }))
            }
            false => div().id(("outline-twistie", index)).size(px(16.)).flex_none(),
        };
        let content = match (row.group, row.element) {
            (Some(label), _) => div().child(label.to_string()),
            (None, Some(element)) => self.outline_element_label(element, (tree_focused && selected).then(|| self.list_selection_icon_color())),
            (None, None) => div(),
        };
        let (selection, selection_foreground) = match tree_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        // listWidget.ts: the focused row is the selected one here, so `listFocusAndSelectionOutline` (high contrast only).
        let focus_outline = self.theme.color("list.focusAndSelectionOutline").or(self.theme.color("contrastActiveBorder")).filter(|_| tree_focused && selected);
        let clicked = id.clone();
        div()
            .id(("outline-row", index))
            .relative()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .cursor_pointer()
            .mx(px(4.))
            .rounded(px(4.))
            // monaco-tl-twistie: 3px row inset and 8px base indent, then the indent per level.
            .pl(px(11. + indent * row.depth as f32))
            .when(!selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .when(selected, |row| row.bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)))
            .when_some(focus_outline, |row, color| row.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color)))
            .child(twistie)
            .child(content.flex_1().min_w_0().flex().items_center())
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.outline_row_clicked(&clicked, event.click_count() > 1, window, cx)))
            .into_any_element()
    }

    /// `icon_color` overrides the symbol color on a selected row of the focused tree.
    fn outline_element_label(&self, element: &OutlineElement, icon_color: Option<gpui::Rgba>) -> Div {
        let symbol = &element.symbol;
        let codicon = crate::symbol_icons::codicon(symbol.kind);
        let icon_color = icon_color.or_else(|| crate::symbol_icons::color_key(codicon).and_then(|key| self.theme.color(key))).unwrap_or(self.color("foreground"));
        let problems = element.marker.filter(|_| self.setting("problems.visibility") != false);
        let problem_color = problems.map(|(_, severity)| self.color(if severity == ERROR { "list.errorForeground" } else { "list.warningForeground" }));
        let label_color = problem_color.filter(|_| self.setting("outline.problems.colors") != false);
        let badge = problems.filter(|_| self.setting("outline.problems.badges") != false).map(|(count, _)| {
            let decoration = div().ml_auto().pl(px(5.)).flex_none().text_color(problem_color.expect("with problems"));
            match count {
                0 => decoration.pr(px(8.)).opacity(0.4).child(self.icon("circle-filled", 14., 14., problem_color.expect("with problems"))),
                _ => decoration.pr(px(12.)).opacity(0.75).text_size(px(11.7)).font_weight(gpui::FontWeight::SEMIBOLD).child(if count < 10 { count.to_string() } else { "+9".into() }),
            }
        });
        let deprecated = symbol.tags.contains(&DEPRECATED);
        div()
            .overflow_hidden()
            .whitespace_nowrap()
            .when(self.setting("outline.icons") != false, |label| label.child(self.icon(codicon, 16., 16., icon_color).mr(px(6.)).flex_none()))
            .child(div().flex_none().when_some(label_color, |name, color| name.text_color(color)).when(deprecated, |name| name.line_through().opacity(0.66)).child(symbol.name.replace('\n', "⏎")))
            .when(!symbol.detail.is_empty(), |label| label.child(div().ml(px(6.5)).text_size(px(11.7)).opacity(0.7).min_w_0().overflow_hidden().text_ellipsis().child(symbol.detail.clone())))
            .children(badge)
    }

    /// `DocumentSymbolFilter`: `<prefix>.show<Kind>` of the symbol's kind, shown when unset.
    fn shows_symbol_kind(&self, prefix: &str, kind: u64) -> bool {
        let Some(name) = SHOW_KIND.get(kind as usize) else { return true };
        self.setting(&format!("{prefix}.{name}")) != false
    }

    /// `DocumentSymbolBreadcrumbsSource._computeBreadcrumbs` with `breadcrumbs.symbolPath`: the symbols around the
    /// cursor from the outermost, up to the first filtered one, as (kind, name); none around the cursor in a
    /// non-empty outline is `…` without a kind (breadcrumbsModel.ts `_getOutlineElements`).
    pub(super) fn breadcrumb_symbols(&self, editor: &crate::editor::OpenEditor) -> Vec<(Option<u64>, String)> {
        let Status::Ready { model_id, outline, .. } = &self.outline_pane.status else { return Vec::new() };
        let symbol_path = self.setting("breadcrumbs.symbolPath");
        if *model_id != editor.model_id || symbol_path == "off" {
            return Vec::new();
        }
        let head = editor.selection().head;
        let position = (head.line + 1, crate::editor::utf16_column(&editor.document().lines[head.line], head.column) + 1);
        let path = outline.item_enclosing_position(position, None).map(|item| outline.path_to(&item.id).expect("an element of the outline")).unwrap_or_default();
        let mut symbols: Vec<(Option<u64>, String)> = path
            .into_iter()
            .take_while(|element| self.shows_symbol_kind("breadcrumbs", element.symbol.kind))
            .map(|element| (Some(element.symbol.kind), element.symbol.name.clone()))
            .collect();
        if symbols.is_empty() && !outline.groups.is_empty() {
            return vec![(None, "\u{2026}".into())];
        }
        if symbol_path == "last" {
            symbols.drain(..symbols.len().saturating_sub(1));
        }
        symbols
    }

    /// listWidget.ts `.monaco-list:focus .monaco-list-row.selected .codicon`: `list.activeSelectionIconForeground`,
    /// unset in the default themes, so the row's foreground.
    pub(super) fn list_selection_icon_color(&self) -> gpui::Rgba {
        self.theme.color("list.activeSelectionIconForeground").or(self.theme.color("list.activeSelectionForeground")).unwrap_or(self.color("foreground"))
    }

    pub(super) fn outline_row_expanded(&self, model_id: u32, id: &str) -> bool {
        let collapsed_by_default = self.setting("outline.collapseItems") == "alwaysCollapse";
        collapsed_by_default == self.outline_pane.toggled.contains(&(model_id, id.to_string()))
    }

    /// Expands or collapses `id`, whatever it shows now.
    pub(super) fn set_outline_row_expanded(&mut self, model_id: u32, id: &str, expanded: bool) {
        let collapsed_by_default = self.setting("outline.collapseItems") == "alwaysCollapse";
        let key = (model_id, id.to_string());
        match expanded == collapsed_by_default {
            true => self.outline_pane.toggled.insert(key),
            false => self.outline_pane.toggled.remove(&key),
        };
    }

    /// outlinePane.ts `revealActiveElement`: with Follow Cursor, the element around the cursor of the shown model
    /// is revealed (its parents expanded) and selected, once per cursor move unless `force`.
    pub(super) fn reveal_outline_active_element(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let head = editor.selection().head;
        let Status::Ready { model_id, outline, .. } = &self.outline_pane.status else { return };
        let followed = Some((*model_id, head));
        if !self.outline_pane.view_state.follow_cursor || *model_id != editor.model_id || (!force && self.outline_pane.followed == followed) {
            return;
        }
        self.outline_pane.followed = followed;
        let position = (head.line + 1, crate::editor::utf16_column(&editor.document().lines[head.line], head.column) + 1);
        let Some(item) = outline.item_enclosing_position(position, None) else { return };
        let path: Vec<String> = outline.path_to(&item.id).expect("an element of the outline").iter().map(|element| element.id.clone()).collect();
        let model_id = *model_id;
        let group = matches!(outline.children, OutlineChildren::Groups).then(|| outline.groups.iter().find(|group| item.id.starts_with(&format!("{}/", group.id))).map(|group| group.id.clone())).flatten();
        for id in group.iter().chain(&path[..path.len() - 1]) {
            self.set_outline_row_expanded(model_id, id, true);
        }
        self.outline_pane.selected = path.last().cloned();
        cx.notify();
    }

    fn toggle_outline_row(&mut self, model_id: u32, id: &str, cx: &mut Context<Self>) {
        let key = (model_id, id.to_string());
        if !self.outline_pane.toggled.remove(&key) {
            self.outline_pane.toggled.insert(key);
        }
        cx.notify();
    }

    /// `onDidOpen`: a click selects the row and opens the symbol at the start of its name keeping the focus in the
    /// tree; a double click selects the symbol's whole range in the focused editor.
    fn outline_row_clicked(&mut self, id: &str, double_click: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.outline_pane.selected = Some(id.to_string());
        window.focus(&self.outline_pane.focus, cx);
        self.reveal_outline_symbol(id, double_click, window, cx);
        cx.notify();
    }

    /// documentSymbolEditor.ts `reveal`: the selection near the top when outside the viewport.
    fn reveal_outline_symbol(&mut self, id: &str, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Status::Ready { model_id, outline, .. } = &self.outline_pane.status else { return };
        let Some(element) = outline.path_to(id).and_then(|path| path.last().copied()) else { return };
        let range = element.symbol.range;
        let (start, end) = match select {
            true => (range.start, range.end),
            false => (element.symbol.selection_range.start, element.symbol.selection_range.start),
        };
        let model_id = *model_id;
        let viewport = self.editors.viewport;
        let Some(editor) = self.editors.active_mut().filter(|editor| editor.model_id == model_id) else { return };
        let (start, end) = (position_of(editor, start), position_of(editor, end));
        editor.set_selections(vec![Selection { anchor: start, head: end }]);
        editor.reveal_range(start, end, VerticalReveal::NearTopIfOutsideViewport, viewport);
        self.host.send_selection(self.editors.active().expect("checked above"), "code.navigation");
        self.editor_scrolled(cx);
        if select {
            window.focus(&self.editor_focus, cx);
        }
    }

    /// The tree's keys: Up and Down move the selection, Left collapses or goes to the parent, Right expands or goes
    /// to the first child, Enter and Space open the selected symbol.
    fn outline_key_down(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.active_notebook_editor().is_some() {
            return false;
        }
        let Status::Ready { model_id, outline, .. } = &self.outline_pane.status else { return false };
        let model_id = *model_id;
        let rows = self.outline_rows(model_id, outline);
        let current = self.outline_pane.selected.as_deref().and_then(|selected| rows.iter().position(|row| row.id == selected));
        let target = match (key, current) {
            ("down", None) | ("home", _) => Some(0),
            ("up", None) | ("end", _) => Some(rows.len().saturating_sub(1)),
            ("down", Some(index)) => Some((index + 1).min(rows.len() - 1)),
            ("up", Some(index)) => Some(index.saturating_sub(1)),
            ("left" | "right", Some(index)) => {
                let row = &rows[index];
                let expanded = self.outline_row_expanded(model_id, row.id);
                match (key, row.has_children && expanded, row.has_children) {
                    ("left", true, _) | ("right", false, true) => {
                        let id = row.id.to_string();
                        self.toggle_outline_row(model_id, &id, cx);
                        return true;
                    }
                    ("left", false, _) => rows[..index].iter().rposition(|parent| parent.depth + 1 == row.depth),
                    _ => rows.get(index + 1).filter(|child| child.depth > row.depth).map(|_| index + 1),
                }
            }
            ("enter" | "space", Some(index)) => {
                let id = rows[index].id.to_string();
                self.reveal_outline_symbol(&id, false, window, cx);
                if key == "enter" {
                    window.focus(&self.editor_focus, cx);
                }
                return true;
            }
            _ => return false,
        };
        if let Some(target) = target.filter(|_| !rows.is_empty()) {
            self.outline_pane.selected = Some(rows[target].id.to_string());
            cx.notify();
        }
        true
    }
}

fn push_elements<'a>(rows: &mut Vec<Row<'a>>, elements: &'a [OutlineElement], depth: usize, sort_by: SortOrder, expanded: &impl Fn(&str) -> bool, shown: &impl Fn(&OutlineElement) -> bool) {
    let mut sorted: Vec<&OutlineElement> = elements.iter().filter(|element| shown(element)).collect();
    sorted.sort_by(|a, b| compare_elements(a, b, sort_by));
    for element in sorted {
        // `hideTwistiesOfChildlessElements`.
        let has_children = !element.children.is_empty();
        rows.push(Row { depth, id: &element.id, group: None, element: Some(element), has_children });
        if has_children && expanded(&element.id) {
            push_elements(rows, &element.children, depth + 1, sort_by, expanded, shown);
        }
    }
}

/// `DocumentSymbolComparator`: names with `Intl.Collator(undefined, { numeric: true })`.
fn compare_elements(a: &OutlineElement, b: &OutlineElement, sort_by: SortOrder) -> std::cmp::Ordering {
    let by_position = || a.symbol.range.start.cmp(&b.symbol.range.start);
    let by_name = || crate::explorer::compare_file_names(&a.symbol.name, &b.symbol.name);
    match sort_by {
        SortOrder::ByPosition => by_position().then_with(by_name),
        SortOrder::ByName => by_name().then_with(by_position),
        SortOrder::ByKind => a.symbol.kind.cmp(&b.symbol.kind).then_with(by_name),
    }
}

#[cfg(test)]
mod tests;
