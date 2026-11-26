//! The Source Control Graph view (scmHistoryViewPane.ts): the "Graph" pane under "Changes" with the history items of
//! the active repository's history provider, their swimlane graph (scmHistory.ts), reference badges and changes.

use std::collections::HashMap;

use gpui::{Bounds, ClickEvent, Context, Div, FontWeight, MouseButton, MouseDownEvent, PathBuilder, Pixels, Point, Rgba, Window, canvas, div, point, prelude::*, px, size};
use serde_json::{Value, json};

use super::Workbench;
use super::scm_view::ROW_HEIGHT;
use super::view_panes::{PaneContainer, SCM_HISTORY_VIEW, ViewPane};
use crate::scm::Provider;
use crate::scm_history::{
    ColorId, HISTORY_ITEM_BASE_REF_COLOR, HISTORY_ITEM_REF_COLOR, HISTORY_ITEM_REMOTE_REF_COLOR, HistoryItem, HistoryItemKind, HistoryItemRef, HistoryItemViewModel, SCM_INCOMING_HISTORY_ITEM_ID,
    SCM_OUTGOING_HISTORY_ITEM_ID, history_item_index, node_color, to_history_item_view_models,
};

const SWIMLANE_HEIGHT: f32 = 22.;
const SWIMLANE_WIDTH: f32 = 11.;
const SWIMLANE_CURVE_RADIUS: f32 = 5.;
const CIRCLE_RADIUS: f32 = 4.;
const CIRCLE_STROKE_WIDTH: f32 = 2.;

/// The current, remote and base refs of `ISCMHistoryProvider`, colored as mainThreadSCM.ts `toISCMHistoryItemRef`.
type CurrentRefs = [Option<HistoryItemRef>; 3];

/// `SCMHistoryViewModel` of the active repository with the 'auto' history item filter (the current refs).
#[derive(Default)]
pub struct ScmGraph {
    /// The provider handle and the `$onDidChangeHistoryProviderCurrentHistoryItemRefs` values the items belong to.
    pub(super) loaded_for: Option<(u64, [Option<Value>; 3])>,
    /// Bumped on every reload, so answers to an older one are dropped.
    generation: u64,
    loading: bool,
    pub(super) refs: CurrentRefs,
    history_items: Vec<HistoryItem>,
    pub(super) merge_base: Option<String>,
    pub(super) view_models: Vec<HistoryItemViewModel>,
    /// The expanded history items with their `SCMHistoryItemChangeDto`s, `None` while they load.
    pub(super) changes: HashMap<String, Option<Vec<Value>>>,
    /// The selected history item's id.
    pub(super) selected: Option<String>,
    pub(super) scroll: gpui::ScrollHandle,
}

impl ScmGraph {
    fn reset(&mut self, loaded_for: Option<(u64, [Option<Value>; 3])>, refs: CurrentRefs) {
        *self = ScmGraph { generation: self.generation + 1, loaded_for, refs, ..ScmGraph::default() };
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    /// The SVG `A r r 0 0 sweep x y` command.
    Arc { radius: f32, sweep: bool, x: f32, y: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct GraphPath {
    pub color: ColorId,
    pub width: f32,
    pub segments: Vec<Segment>,
}

impl Workbench {
    /// Reloads the graph when the active repository or its current history item refs change (the
    /// `historyItemRef`/`historyItemRemoteRef`/`historyItemBaseRef` autorun of `SCMHistoryViewPane`).
    pub(super) fn update_scm_graph(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.active_scm_provider().filter(|provider| provider.has_history_provider) else {
            if self.scm_graph.loaded_for.is_some() {
                self.scm_graph.reset(None, CurrentRefs::default());
            }
            return;
        };
        if !self.view_pane_expanded(PaneContainer::Scm, SCM_HISTORY_VIEW) {
            return;
        }
        let loaded_for = (provider.handle, [provider.history_item_ref.clone(), provider.history_item_remote_ref.clone(), provider.history_item_base_ref.clone()]);
        if self.scm_graph.loaded_for.as_ref() == Some(&loaded_for) {
            return;
        }
        let refs = current_refs(provider);
        let handle = provider.handle;
        self.scm_graph.reset(Some(loaded_for), refs);
        self.load_scm_graph_page(handle, cx);
    }

    /// `SCMHistoryViewModel.getHistoryItems`: the next page, and the merge base of the current and remote refs
    /// with the first one.
    fn load_scm_graph_page(&mut self, handle: u64, cx: &mut Context<Self>) {
        let limit = self.setting("scm.graph.pageSize").as_u64().unwrap_or(50).clamp(1, 1000);
        let graph = &mut self.scm_graph;
        graph.loading = true;
        let options = history_options(&graph.refs, limit, graph.history_items.len());
        let items = self.host.call_cancellable("ExtHostSCM", "$provideHistoryItems", vec![json!(handle), options]);
        let merge_base = match &graph.refs {
            [Some(current), Some(remote), _] if graph.history_items.is_empty() => {
                Some(self.host.call_cancellable("ExtHostSCM", "$resolveHistoryItemRefsCommonAncestor", vec![json!(handle), json!([current.name, remote.name])]))
            }
            _ => None,
        };
        let generation = graph.generation;
        cx.spawn(async move |workbench, cx| {
            let page = items.await.ok().and_then(Result::ok).unwrap_or_default();
            let merge_base = match merge_base {
                Some(call) => call.await.ok().and_then(Result::ok).and_then(|base| base.as_str().map(str::to_string)),
                None => None,
            };
            let _ = workbench.update(cx, |workbench, cx| {
                if workbench.scm_graph.generation == generation {
                    workbench.add_scm_graph_page(&page, merge_base);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn add_scm_graph_page(&mut self, page: &Value, merge_base: Option<String>) {
        let show_incoming = self.setting("scm.graph.showIncomingChanges") != json!(false);
        let show_outgoing = self.setting("scm.graph.showOutgoingChanges") != json!(false);
        let graph = &mut self.scm_graph;
        graph.loading = false;
        graph.history_items.extend(page.as_array().into_iter().flatten().filter_map(HistoryItem::from_dto));
        if merge_base.is_some() {
            graph.merge_base = merge_base;
        }
        let [current, remote, base] = &graph.refs;
        graph.view_models = to_history_item_view_models(
            &graph.history_items,
            &graph_color_map(&graph.refs),
            current.as_ref(),
            remote.as_ref(),
            base.as_ref(),
            show_incoming && remote.is_some(),
            show_outgoing && current.is_some(),
            graph.merge_base.as_deref(),
        );
    }

    /// A click on a history item: collapses it, or expands it with `$provideHistoryItemChanges`.
    fn toggle_scm_history_item(&mut self, handle: u64, index: usize, cx: &mut Context<Self>) {
        let graph = &mut self.scm_graph;
        let view_model = &graph.view_models[index];
        let id = view_model.history_item.id.clone();
        cx.notify();
        if graph.changes.remove(&id).is_some() {
            return;
        }
        let Some((item_id, parent_id)) = changes_request(view_model, &graph.refs, graph.merge_base.as_deref()) else { return };
        graph.changes.insert(id.clone(), None);
        let generation = graph.generation;
        let call = self.host.call_cancellable("ExtHostSCM", "$provideHistoryItemChanges", vec![json!(handle), json!(item_id), json!(parent_id)]);
        cx.spawn(async move |workbench, cx| {
            let changes = call.await.ok().and_then(Result::ok).and_then(|changes| changes.as_array().cloned()).unwrap_or_default();
            let _ = workbench.update(cx, |workbench, cx| {
                let graph = &mut workbench.scm_graph;
                if graph.generation == generation
                    && let Some(entry) = graph.changes.get_mut(&id)
                {
                    *entry = Some(changes);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// `SCMHistoryViewPane._onDidOpen` of a change: the diff of both sides, else the side there is.
    fn open_scm_history_change(&mut self, history_item: &HistoryItem, change: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let side = |key: &str| Some(change[key].clone()).filter(|uri| uri.is_object());
        match (side("originalUri"), side("modifiedUri")) {
            (Some(original), Some(modified)) => {
                let title = change_title(history_item, &original, &modified);
                drop(self.execute_command("_workbench.diff", vec![original, modified, json!(title)], window, cx));
            }
            (Some(uri), None) | (None, Some(uri)) => drop(self.execute_command("vscode.open", vec![uri], window, cx)),
            (None, None) => {}
        }
    }

    /// The Graph pane: the history items with their expanded changes and "Load More".
    pub(super) fn scm_graph_pane(&self, provider: &Provider, window: &Window, cx: &mut Context<Self>) -> ViewPane {
        let graph = &self.scm_graph;
        let handle = provider.handle;
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for (index, view_model) in graph.view_models.iter().enumerate() {
            let changes = graph.changes.get(&view_model.history_item.id);
            rows.push(self.scm_history_item_row(handle, index, view_model, changes.is_some(), cx).into_any_element());
            for (change_index, change) in changes.into_iter().flatten().flatten().enumerate() {
                rows.push(self.scm_history_change_row(provider, view_model, change_index, change, cx).into_any_element());
            }
        }
        if let Some(last) = graph.view_models.last().filter(|_| has_more(&graph.view_models)) {
            rows.push(self.scm_graph_load_more_row(handle, last, cx).into_any_element());
        }
        let body = div().id("scm-graph-list").flex_1().min_h_0().overflow_y_scroll().track_scroll(&graph.scroll).children(rows).into_any_element();
        // scmHistoryViewPane.ts `ViewPaneShowActions.WhenExpanded`: the actions show while the pane is expanded.
        ViewPane { id: SCM_HISTORY_VIEW, title: "Graph".into(), description: None, actions: Some(self.scm_graph_title_actions(provider, window, cx).into_any_element()), show_actions: true, body }
    }

    /// `HistoryItemRenderer.renderElement`: the graph, the subject with the author as description, the badges.
    fn scm_history_item_row(&self, handle: u64, index: usize, view_model: &HistoryItemViewModel, expanded: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let history_item = &view_model.history_item;
        let mut paths = graph_paths(view_model);
        // scm.css: the last path of an expanded item is 3px wide.
        if let Some(last) = paths.last_mut().filter(|_| expanded) {
            last.width = 3.;
        }
        let circle_x = SWIMLANE_WIDTH * (history_item_index(view_model) + 1) as f32;
        let dashed = matches!(view_model.kind, HistoryItemKind::IncomingChanges | HistoryItemKind::OutgoingChanges);
        let circle_color = node_color(&history_item.id, &view_model.input_swimlanes, &view_model.output_swimlanes);
        let circle = Circle { x: circle_x, discs: self.resolve_discs(&graph_discs(view_model)), dashed: dashed.then(|| self.color(circle_color)) };
        let lanes = view_model.input_swimlanes.len().max(view_model.output_swimlanes.len()).max(1);
        let is_head = self.scm_graph.refs[0].as_ref().and_then(|current| current.revision.as_deref()) == Some(history_item.id.as_str());
        let all_badges = self.setting("scm.graph.badges").as_str() == Some("all");
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(div().flex_none().when(is_head, |name| name.font_weight(FontWeight::SEMIBOLD)).child(history_item.subject.clone()))
            .children(history_item.author.clone().map(|author| div().ml(px(6.)).text_size(px(12.)).opacity(0.7).child(author)));
        let badges = badges(&history_item.references, all_badges).into_iter().filter_map(|(reference, count, show_name)| self.scm_history_badge(reference, count, show_name));
        let selected = self.scm_graph.selected.as_ref() == Some(&history_item.id);
        div()
            .id(("scm-history-item", index))
            .h(px(ROW_HEIGHT))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| workbench.toggle_scm_history_item(handle, index, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_scm_history_item_menu(handle, index, event.position, window, cx)))
            .child(graph_canvas(self.resolve_paths(&paths), Some(circle), SWIMLANE_WIDTH * (lanes + 1) as f32))
            .child(label)
            .child(div().flex_none().ml(px(4.)).flex().gap(px(4.)).children(badges))
    }

    /// `HistoryItemRenderer._renderBadge`: the ref's color as background, the count, the icon and the name.
    fn scm_history_badge(&self, reference: &HistoryItemRef, count: usize, show_name: bool) -> Option<Div> {
        let icon = theme_icon(reference)?;
        let (foreground, background) = match reference.color {
            Some(color) => (self.color("scmGraph.historyItemHoverLabelForeground"), self.color(color)),
            None => (self.color("foreground"), self.color("scmGraph.historyItemHoverDefaultLabelBackground")),
        };
        let icon_size = if icon == "git-branch" { 12. } else { 16. };
        Some(
            div()
                .h(px(18.))
                .flex()
                .items_center()
                .rounded(px(10.))
                .bg(background)
                .text_color(foreground)
                .text_size(px(12.))
                .when(count > 1, |badge| badge.child(div().pl(px(4.)).child(count.to_string())))
                .child(self.icon(icon, 18., icon_size, foreground))
                .when(show_name, |badge| badge.child(div().pr(px(4.)).max_w(px(100.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(reference.name.clone()))),
        )
    }

    /// `HistoryItemChangeRenderer` in list mode: the graph columns, the file icon, the name and its folder.
    fn scm_history_change_row(&self, provider: &Provider, view_model: &HistoryItemViewModel, change_index: usize, change: &Value, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let path = std::path::PathBuf::from(change["uri"]["path"].as_str().unwrap_or_default());
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let root = provider.root_uri["path"].as_str().unwrap_or_default();
        let folder = path.parent().and_then(|parent| parent.strip_prefix(root).ok()).map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default();
        let language = self.languages.language_of(&path, "").to_string();
        let icon = self.file_icons.as_ref().and_then(|icons| icons.file_icon(&path, &language, self.theme.kind, 13., ROW_HEIGHT, foreground)).unwrap_or_else(|| div().w(px(16.)));
        let columns = &view_model.output_swimlanes;
        let graph_width = SWIMLANE_WIDTH * (columns.len() + 1) as f32;
        let placeholder = graph_canvas(self.resolve_paths(&placeholder_paths(columns, Some(history_item_index(view_model)))), None, graph_width);
        let history_item = view_model.history_item.clone();
        let opened = change.clone();
        let (menu_item, menu_change) = (history_item.clone(), change.clone());
        div()
            .id(gpui::SharedString::from(format!("scm-history-change-{}-{change_index}", history_item.id)))
            .h(px(ROW_HEIGHT))
            .pr(px(12.))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.open_scm_history_change(&history_item, &opened, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_scm_history_change_menu(&menu_item, &menu_change, event.position, window, cx)))
            .child(placeholder)
            .child(icon.ml(px(16.)).mr(px(6.)))
            .child(div().flex_none().whitespace_nowrap().child(name))
            .child(div().flex_1().min_w_0().ml(px(6.)).overflow_hidden().whitespace_nowrap().text_size(px(12.)).opacity(0.7).child(folder))
    }

    /// `HistoryItemLoadMoreRenderer` with `scm.graph.pageOnScroll` off: the last item's columns and "Load More...".
    fn scm_graph_load_more_row(&self, handle: u64, last: &HistoryItemViewModel, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let foreground = self.foreground_or("sideBar.foreground");
        let columns = &last.output_swimlanes;
        let placeholder = graph_canvas(self.resolve_paths(&placeholder_paths(columns, None)), None, SWIMLANE_WIDTH * (columns.len() + 1) as f32);
        let icon = if self.scm_graph.loading { "loading" } else { "fold-down" };
        div()
            .id("scm-graph-load-more")
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                if !workbench.scm_graph.loading {
                    workbench.load_scm_graph_page(handle, cx);
                    cx.notify();
                }
            }))
            .child(placeholder)
            .child(self.icon(icon, 16., 16., foreground).mr(px(4.)))
            .child("Load More...")
    }

    fn resolve_paths(&self, paths: &[GraphPath]) -> Vec<(Rgba, f32, Vec<Segment>)> {
        paths.iter().map(|path| (self.color(path.color), path.width, path.segments.clone())).collect()
    }

    /// The discs with their colors; the circle strokes are the side bar background (scm.css).
    fn resolve_discs(&self, discs: &[(f32, Option<ColorId>)]) -> Vec<(f32, Rgba)> {
        discs.iter().map(|(radius, color)| (*radius, self.color(color.unwrap_or("sideBar.background")))).collect()
    }
}

/// The resolved circle of a row: its center x, its discs outermost first and the dashed ring's color.
struct Circle {
    x: f32,
    discs: Vec<(f32, Rgba)>,
    dashed: Option<Rgba>,
}

fn graph_canvas(paths: Vec<(Rgba, f32, Vec<Segment>)>, circle: Option<Circle>, width: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
            for (color, width, segments) in &paths {
                paint_stroke(window, PathBuilder::stroke(px(*width)), segments, at, *color);
            }
            let Some(circle) = &circle else { return };
            for (radius, color) in circle.discs.iter().filter(|(radius, _)| *radius > 0.) {
                let corner = at(circle.x - radius, SWIMLANE_WIDTH - radius);
                window.paint_quad(gpui::fill(Bounds::new(corner, size(px(radius * 2.), px(radius * 2.))), *color).corner_radii(px(*radius)));
            }
            if let Some(color) = circle.dashed {
                let radius = CIRCLE_RADIUS + 1.;
                let ring = [
                    Segment::Move(circle.x + radius, SWIMLANE_WIDTH),
                    Segment::Arc { radius, sweep: true, x: circle.x - radius, y: SWIMLANE_WIDTH },
                    Segment::Arc { radius, sweep: true, x: circle.x + radius, y: SWIMLANE_WIDTH },
                ];
                paint_stroke(window, PathBuilder::stroke(px(CIRCLE_STROKE_WIDTH - 1.)).dash_array(&[px(4.), px(2.)]), &ring, at, color);
            }
        },
    )
    .flex_none()
    .w(px(width))
    .h(px(SWIMLANE_HEIGHT))
}

fn paint_stroke(window: &mut Window, mut builder: PathBuilder, segments: &[Segment], at: impl Fn(f32, f32) -> Point<Pixels>, color: Rgba) {
    for segment in segments {
        match *segment {
            Segment::Move(x, y) => builder.move_to(at(x, y)),
            Segment::Line(x, y) => builder.line_to(at(x, y)),
            Segment::Arc { radius, sweep, x, y } => builder.arc_to(point(px(radius), px(radius)), px(0.), false, sweep, at(x, y)),
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn current_refs(provider: &Provider) -> CurrentRefs {
    let colored = |dto: &Option<Value>, color: ColorId| dto.as_ref().and_then(HistoryItemRef::from_dto).map(|reference| HistoryItemRef { color: Some(color), ..reference });
    [
        colored(&provider.history_item_ref, HISTORY_ITEM_REF_COLOR),
        colored(&provider.history_item_remote_ref, HISTORY_ITEM_REMOTE_REF_COLOR),
        colored(&provider.history_item_base_ref, HISTORY_ITEM_BASE_REF_COLOR),
    ]
}

/// The `ISCMHistoryOptions` of `SCMHistoryViewModel.getHistoryItems`: the filter refs' revisions, a page and the
/// items already loaded to skip.
pub(super) fn history_options(refs: &CurrentRefs, limit: u64, skip: usize) -> Value {
    let ids: Vec<&str> = refs.iter().flatten().map(|reference| reference.revision.as_deref().unwrap_or(&reference.id)).collect();
    json!({ "historyItemRefs": ids, "limit": limit, "skip": skip })
}

/// `SCMHistoryViewModel._getGraphColorMap` of the 'auto' filter: the refs keep their color while there is a
/// current ref, else they take their item's.
fn graph_color_map(refs: &CurrentRefs) -> HashMap<String, Option<ColorId>> {
    let has_current = refs[0].is_some();
    refs.iter().flatten().map(|reference| (reference.id.clone(), reference.color.filter(|_| has_current))).collect()
}

/// `SCMHistoryTreeDataSource.getChildren`: a "Load More" row follows a last item that still has parents to show.
pub(super) fn has_more(view_models: &[HistoryItemViewModel]) -> bool {
    view_models.last().is_some_and(|last| !last.output_swimlanes.is_empty())
}

/// `SCMHistoryTreeDataSource.getChildren` of a history item: the item and parent ids of `$provideHistoryItemChanges`;
/// incoming and outgoing changes compare the remote or current ref with the merge base.
pub(super) fn changes_request(view_model: &HistoryItemViewModel, refs: &CurrentRefs, merge_base: Option<&str>) -> Option<(String, Option<String>)> {
    let history_item = &view_model.history_item;
    match view_model.kind {
        HistoryItemKind::IncomingChanges | HistoryItemKind::OutgoingChanges => {
            let (current, remote) = (refs[0].as_ref()?, refs[1].as_ref()?);
            let id = if view_model.kind == HistoryItemKind::IncomingChanges { &remote.id } else { &current.id };
            Some((id.clone(), merge_base.map(str::to_string)))
        }
        HistoryItemKind::Head | HistoryItemKind::Node => Some((history_item.id.clone(), history_item.parent_ids.first().cloned())),
    }
}

/// The diff editor label of `SCMHistoryViewPane._onDidOpen`: `name (parent id) ↔ name (id)`.
fn change_title(history_item: &HistoryItem, original: &Value, modified: &Value) -> String {
    let base_name = |uri: &Value| uri["path"].as_str().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_string();
    let display_id = match history_item.id.as_str() {
        SCM_INCOMING_HISTORY_ITEM_ID => "Incoming Changes".to_string(),
        SCM_OUTGOING_HISTORY_ITEM_ID => "Outgoing Changes".to_string(),
        id => history_item.display_id.clone().unwrap_or_else(|| id.to_string()),
    };
    let parent_id = history_item.parent_ids.first().cloned().unwrap_or_default();
    let parent_display_id = match &history_item.display_id {
        Some(display_id) => parent_id.chars().take(display_id.chars().count()).collect(),
        None => parent_id,
    };
    format!("{} ({parent_display_id}) \u{2194} {} ({display_id})", base_name(original), base_name(modified))
}

/// `HistoryItemRenderer._renderBadges`: the first ref named if colored, then one badge per color and icon with
/// its count; refs without a color only with `scm.graph.badges` `all`. Each badge is its first ref, the count and
/// whether it shows the name.
pub(super) fn badges(references: &[HistoryItemRef], all: bool) -> Vec<(&HistoryItemRef, usize, bool)> {
    let mut badges = Vec::new();
    let mut rest = references;
    if let Some(first) = references.first().filter(|first| first.color.is_some()) {
        badges.push((first, 1, true));
        rest = &references[1..];
    }
    let mut colors: Vec<Option<ColorId>> = Vec::new();
    for reference in rest {
        if !colors.contains(&reference.color) {
            colors.push(reference.color);
        }
    }
    for color in colors.into_iter().filter(|color| color.is_some() || all) {
        let mut icons: Vec<&str> = Vec::new();
        for icon in rest.iter().filter(|reference| reference.color == color).filter_map(theme_icon) {
            if !icons.contains(&icon) {
                icons.push(icon);
            }
        }
        for icon in icons {
            let group: Vec<&HistoryItemRef> = rest.iter().filter(|reference| reference.color == color && theme_icon(reference) == Some(icon)).collect();
            badges.push((group[0], group.len(), false));
        }
    }
    badges
}

/// The codicon id of a ref's `ThemeIcon`, `None` for an icon given by uri.
fn theme_icon(reference: &HistoryItemRef) -> Option<&str> {
    reference.icon.as_ref()?["id"].as_str()
}

/// scmHistory.ts `renderSCMHistoryItemGraph`: the lane paths of one row, in paint order.
pub(super) fn graph_paths(view_model: &HistoryItemViewModel) -> Vec<GraphPath> {
    use Segment::*;
    let history_item = &view_model.history_item;
    let (input, output) = (&view_model.input_swimlanes, &view_model.output_swimlanes);
    let input_index = input.iter().position(|node| node.id == history_item.id);
    let circle_index = history_item_index(view_model);
    let circle_color = node_color(&history_item.id, input, output);
    let x = |index: usize| SWIMLANE_WIDTH * (index + 1) as f32;
    let middle = SWIMLANE_HEIGHT / 2.;
    let path = |color: ColorId, segments: Vec<Segment>| GraphPath { color, width: 1., segments };
    let mut paths = Vec::new();
    let mut output_index = 0;
    for (index, node) in input.iter().enumerate() {
        if node.id == history_item.id {
            if index == circle_index {
                output_index += 1;
            } else {
                let base = Arc { radius: SWIMLANE_WIDTH, sweep: true, x: SWIMLANE_WIDTH * index as f32, y: SWIMLANE_WIDTH };
                paths.push(path(node.color, vec![Move(x(index), 0.), base, Line(x(circle_index), SWIMLANE_WIDTH)]));
            }
            continue;
        }
        if output.get(output_index).is_none_or(|next| next.id != node.id) {
            continue;
        }
        if index == output_index {
            paths.push(path(node.color, vec![Move(x(index), 0.), Line(x(index), SWIMLANE_HEIGHT)]));
        } else {
            paths.push(path(
                node.color,
                vec![
                    Move(x(index), 0.),
                    Line(x(index), 6.),
                    Arc { radius: SWIMLANE_CURVE_RADIUS, sweep: true, x: x(index) - SWIMLANE_CURVE_RADIUS, y: middle },
                    Line(x(output_index) + SWIMLANE_CURVE_RADIUS, middle),
                    Arc { radius: SWIMLANE_CURVE_RADIUS, sweep: false, x: x(output_index), y: middle + SWIMLANE_CURVE_RADIUS },
                    Line(x(output_index), SWIMLANE_HEIGHT),
                ],
            ));
        }
        output_index += 1;
    }
    for parent_id in history_item.parent_ids.iter().skip(1) {
        let Some(parent_index) = output.iter().rposition(|node| &node.id == parent_id) else { continue };
        let start = Move(SWIMLANE_WIDTH * parent_index as f32, middle);
        let curve = Arc { radius: SWIMLANE_WIDTH, sweep: true, x: x(parent_index), y: SWIMLANE_HEIGHT };
        paths.push(path(output[parent_index].color, vec![start, curve, start, Line(x(circle_index), middle)]));
    }
    if let Some(index) = input_index {
        paths.push(path(input[index].color, vec![Move(x(circle_index), 0.), Line(x(circle_index), middle)]));
    }
    if !history_item.parent_ids.is_empty() {
        paths.push(path(circle_color, vec![Move(x(circle_index), middle), Line(x(circle_index), SWIMLANE_HEIGHT)]));
    }
    paths
}

/// scmHistory.ts `renderSCMHistoryGraphPlaceholder`: a line per column, the highlighted one 3px wide.
fn placeholder_paths(columns: &[crate::scm_history::GraphNode], highlight: Option<usize>) -> Vec<GraphPath> {
    let mut paths = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let x = SWIMLANE_WIDTH * (index + 1) as f32;
        let width = if Some(index) == highlight { 3. } else { 1. };
        paths.push(GraphPath { color: column.color, width, segments: vec![Segment::Move(x, 0.), Segment::Line(x, SWIMLANE_HEIGHT)] });
    }
    paths
}

/// The circles of `renderSCMHistoryItemGraph` as the discs they paint, outermost first: a radius and a color,
/// `None` for the side bar background of the circle strokes (scm.css). The dashed ring of incoming and outgoing
/// changes is painted over them.
fn graph_discs(view_model: &HistoryItemViewModel) -> Vec<(f32, Option<ColorId>)> {
    let color = Some(node_color(&view_model.history_item.id, &view_model.input_swimlanes, &view_model.output_swimlanes));
    let circle = |radius: f32, stroke: f32, fill: Option<ColorId>| [(radius + stroke / 2., None), (radius - stroke / 2., fill)];
    match view_model.kind {
        HistoryItemKind::Head => [circle(CIRCLE_RADIUS + 3., CIRCLE_STROKE_WIDTH, color), circle(CIRCLE_STROKE_WIDTH, CIRCLE_RADIUS, None)].concat(),
        HistoryItemKind::IncomingChanges | HistoryItemKind::OutgoingChanges => vec![(CIRCLE_RADIUS + 4., None)],
        HistoryItemKind::Node if view_model.history_item.parent_ids.len() > 1 => {
            [circle(CIRCLE_RADIUS + 2., CIRCLE_STROKE_WIDTH, color), circle(CIRCLE_RADIUS - 1., CIRCLE_STROKE_WIDTH, color)].concat()
        }
        HistoryItemKind::Node => circle(CIRCLE_RADIUS + 1., CIRCLE_STROKE_WIDTH, color).to_vec(),
    }
}

#[cfg(test)]
mod tests;
