//! hideUnchangedRegionsFeature.ts: with `diffEditor.hideUnchangedRegions.enabled` the unchanged regions of the
//! active diff are hidden areas of the modified editor and skipped lines of the original pane, each with the
//! `CollapsedCodeOverlayWidget` placeholder: "N hidden lines", the breadcrumbs of the hidden range, the unfold link,
//! and the top and bottom bars that show 20 more lines on click or follow a drag.

use gpui::{Context, CursorStyle, DispatchPhase, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, canvas, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use crate::commands::Outcome;
use crate::diff_editor::unchanged_regions::{OutlineSymbol, RevealPreference, UnchangedDrag, breadcrumb_items, flatten_symbols};
use crate::diff_editor::{DiffInput, ZoneSide};
use crate::editor::{OpenEditor, Position};
use crate::workbench::hover::{HoverContent, HoverOptions};

/// `PlaceholderViewZone` height of the non-compact widget.
pub(super) const PLACEHOLDER_HEIGHT: f32 = 24.;
/// style.css `.diff-hidden-lines .top` and `.bottom`: 4px border, 4px bar, 2px border, moved up 6px (top) and
/// down 18px (bottom) from the zone's top.
const BAR_BOX: f32 = 10.;

impl Workbench {
    pub(in crate::workbench) fn register_unchanged_region_commands(&mut self) {
        self.commands.register("diffEditor.collapseAllUnchangedRegions", |workbench, _, _, cx| {
            workbench.change_unchanged_regions(|input| input.unchanged.iter_mut().for_each(|region| region.collapse_all()), cx);
            Outcome::done()
        });
        self.commands.register("diffEditor.showAllUnchangedRegions", |workbench, _, _, cx| {
            workbench.change_unchanged_regions(|input| input.unchanged.iter_mut().for_each(|region| region.show_all()), cx);
            Outcome::done()
        });
    }

    /// diffEditorOptions.ts `hideUnchangedRegions`, `hideUnchangedRegionsMinimumLineCount` and `...ContextLineCount`.
    pub(super) fn hide_unchanged_regions(&self) -> Option<(usize, usize)> {
        if self.setting("diffEditor.hideUnchangedRegions.enabled") != json!(true) {
            return None;
        }
        let count = |key: &str| self.setting(key).as_u64().unwrap_or(3) as usize;
        Some((count("diffEditor.hideUnchangedRegions.minimumLineCount"), count("diffEditor.hideUnchangedRegions.contextLineCount")))
    }

    fn reveal_line_count(&self) -> usize {
        self.setting("diffEditor.hideUnchangedRegions.revealLineCount").as_u64().unwrap_or(20) as usize
    }

    /// The hidden areas and placeholder view zones of the active diff's regions in its modified editor. A cursor
    /// that moved onto a hidden line reveals it first (`ensureModifiedLineIsVisible` on cursor change).
    pub(super) fn place_unchanged_regions(&mut self, input: &mut DiffInput) {
        let Some(modified) = self.editors.model_mut(&input.modified.scheme, &input.modified.path) else { return };
        let cursors: Vec<usize> = modified.selections.iter().flat_map(|selection| [selection.start().line + 1, selection.end().line + 1]).collect();
        // The cursor the editor opened with is no cursor change.
        if input.unchanged_cursors.is_empty() {
            input.unchanged_cursors = cursors.clone();
        }
        if cursors != input.unchanged_cursors {
            for &line in &cursors {
                if let Some(region) = input.unchanged.iter_mut().find(|region| region.hidden_modified().contains(line)) {
                    region.show_modified_line(line, RevealPreference::FromCloserSide);
                }
            }
            input.unchanged_cursors = cursors;
        }
        modified.folding.set_hidden_areas(input.unchanged.iter().map(|region| zero_based(region.hidden_modified())).collect());
        for (index, region) in input.unchanged.iter().enumerate().filter(|(_, region)| !region.is_fully_revealed()) {
            let after_line = region.hidden_modified().start - 1;
            let anchor = after_line.checked_sub(1).map(|line| Position::new(line, 0));
            let id = modified.add_view_zone(anchor, PLACEHOLDER_HEIGHT);
            input.placed_unchanged.push((id, index));
        }
    }

    /// The diffEditorBreadcrumbs source: the outline of the active diff's modified model (every document symbol
    /// provider's symbols), asked again when the model changes while regions are collapsed.
    pub(in crate::workbench) fn update_diff_symbols(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editors.active_tab().map(|tab| tab.model_id) else { return };
        let Some(input) = self.editors.diffs.get(&id).filter(|input| !input.unchanged.is_empty()) else { return };
        let Some(modified) = self.editors.model(&input.modified.scheme, &input.modified.path) else { return };
        let version = modified.version_id;
        if input.symbols_version == Some(version) {
            return;
        }
        let uri = modified.document.uri();
        let candidate = crate::language_features::Candidate { uri: &uri, language: &modified.document.language_id };
        let calls: Vec<_> = self
            .language_features
            .ordered("$registerDocumentSymbolProvider", &candidate)
            .iter()
            .map(|registration| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentSymbols", vec![json!(registration.handle), uri.clone(), json!({})]))
            .collect();
        self.editors.diffs.get_mut(&id).expect("found above").symbols_version = Some(version);
        cx.spawn(async move |workbench, cx| {
            let mut symbols: Vec<OutlineSymbol> = Vec::new();
            for call in calls {
                if let Ok(Ok(answer)) = call.await {
                    flatten_symbols(&answer, &mut symbols);
                }
            }
            let _ = workbench.update(cx, |workbench, cx| {
                if let Some(input) = workbench.editors.diffs.get_mut(&id).filter(|input| input.symbols_version == Some(version)) {
                    input.symbols = symbols;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn change_unchanged_regions(&mut self, change: impl FnOnce(&mut DiffInput), cx: &mut Context<Self>) {
        let Some(id) = self.editors.active_tab().map(|tab| tab.model_id) else { return };
        let Some(input) = self.editors.diffs.get_mut(&id) else { return };
        change(input);
        cx.notify();
    }

    /// `_runWithLowerScrollAnchor`: lines shown above the bottom of a hidden range keep the lines below it in place.
    fn set_visible_bottom(&mut self, region: usize, count: usize, cx: &mut Context<Self>) {
        let mut added = 0.;
        self.change_unchanged_regions(
            |input| {
                let region = &mut input.unchanged[region];
                let before = region.visible_bottom;
                region.visible_bottom = count.min(region.line_count - region.visible_top);
                added = region.visible_bottom as f32 - before as f32;
            },
            cx,
        );
        let viewport = self.editors.viewport;
        if let Some(editor) = self.active_diff_modified_mut() {
            let (top, left) = (editor.scroll.top + added * viewport.line_height, editor.scroll.left);
            editor.set_scroll(top, left, viewport);
        }
    }

    fn active_diff_modified_mut(&mut self) -> Option<&mut OpenEditor> {
        let input = self.editors.active_diff()?;
        let (scheme, path) = (input.modified.scheme.clone(), input.modified.path.clone());
        self.editors.model_mut(&scheme, &path)
    }

    /// The `diff-hidden-lines` widget of region `index` on one side, as tall as its view zone; `hide` (the original
    /// side inline) leaves only the bar.
    pub(super) fn unchanged_placeholder(&self, input: &DiffInput, index: usize, side: ZoneSide, content_left: f32, hide: bool, cx: &mut Context<Self>) -> Div {
        let region = input.unchanged[index];
        let id = |part: &str| gpui::SharedString::from(format!("diff-hidden-lines-{side:?}-{index}-{part}"));
        let focus = self.color("focusBorder");
        let dragged = input.unchanged_drag.filter(|drag| drag.region == index);
        let can_move_top = |top: bool| if top { region.visible_top > 0 } else { !region.is_fully_revealed() };
        let can_move_bottom = |top: bool| if top { !region.is_fully_revealed() } else { region.visible_bottom > 0 };
        let bar = |top: bool| {
            let cursor = match (can_move_top(top), can_move_bottom(top)) {
                (true, true) => CursorStyle::ResizeUpDown,
                (true, false) => CursorStyle::ResizeUp,
                (false, true) => CursorStyle::ResizeDown,
                (false, false) => CursorStyle::Arrow,
            };
            let title = if top { "Click or drag to show more above" } else { "Click or drag to show more below" };
            let fill = div().absolute().left_0().right_0().top(px(4.)).h(px(4.));
            let fill = match dragged.filter(|drag| drag.top == top) {
                Some(_) => fill.bg(focus),
                None if dragged.is_none() => fill.group_hover(format!("bar-{side:?}-{index}-{top}"), |fill| fill.bg(focus)),
                None => fill,
            };
            div()
                .id(id(if top { "top" } else { "bottom" }))
                .group(format!("bar-{side:?}-{index}-{top}"))
                .absolute()
                .left_0()
                .right_0()
                .top(px(if top { -6. } else { PLACEHOLDER_HEIGHT - 6. }))
                .h(px(BAR_BOX))
                .cursor(cursor)
                .child(fill)
                .child(self.hover_target(id(if top { "top-hover" } else { "bottom-hover" }), HoverContent::Text(title.into()), HoverOptions::mouse()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let start_count = if top { region.visible_top } else { region.visible_bottom };
                        let drag = UnchangedDrag { region: index, top, start_y: f32::from(event.position.y), start_count, moved: false };
                        workbench.change_unchanged_regions(|input| input.unchanged_drag = Some(drag), cx);
                    }),
                )
        };
        let (top_bar, bottom_bar) = (bar(true), bar(false));
        let shadow = self.color("diffEditor.unchangedRegionShadow");
        let mut center = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(PLACEHOLDER_HEIGHT))
            .flex()
            .overflow_hidden()
            .bg(self.color("diffEditor.unchangedRegionBackground"))
            .text_color(self.color("diffEditor.unchangedRegionForeground"))
            .text_size(px(13.))
            .line_height(px(14.))
            .child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(gpui::Rgba { a: shadow.a * 0.3, ..shadow }))
            .child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(gpui::Rgba { a: shadow.a * 0.3, ..shadow }));
        if !hide {
            let unfold = div()
                .id(id("unfold"))
                .cursor_pointer()
                .child(self.icon("unfold", 16., 16., self.color("diffEditor.unchangedRegionForeground")))
                .hover(|link| link.text_color(self.color("editorLink.activeForeground")))
                .child(self.hover_target(id("unfold-hover"), HoverContent::Text("Show Unchanged Region".into()), HoverOptions::mouse()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        workbench.change_unchanged_regions(|input| input.unchanged[index].show_all(), cx);
                    }),
                );
            let hidden_count = region.hidden_modified().len();
            let label = div()
                .id(id("label"))
                .child(format!("{hidden_count} hidden lines"))
                .child(self.hover_target(id("label-hover"), HoverContent::Text("Double click to unfold".into()), HoverOptions::mouse()))
                .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, _, cx| {
                    if event.click_count() == 2 {
                        workbench.change_unchanged_regions(|input| input.unchanged[index].show_all(), cx);
                    }
                }));
            let row = div().flex_1().min_w_0().flex().justify_center().items_center().whitespace_nowrap().child(label).children(self.unchanged_breadcrumbs(input, index, cx));
            center = center.child(div().w(px(content_left)).flex_none().flex().items_center().justify_center().child(unfold)).child(row);
        }
        div().relative().h(px(PLACEHOLDER_HEIGHT)).flex_none().child(center).child(top_bar).child(bottom_bar).when(dragged.is_some(), |zone| zone.child(self.unchanged_drag_listener(cx)))
    }

    /// The breadcrumbs of the hidden range (diffEditorBreadcrumbs contribution): the symbols starting in it and ending
    /// after it, outermost first, each revealing its line on click.
    fn unchanged_breadcrumbs(&self, input: &DiffInput, index: usize, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let region = input.unchanged[index];
        let items = if region.is_fully_revealed() { Vec::new() } else { breadcrumb_items(&input.symbols, region.hidden_modified()) };
        if items.is_empty() {
            return Vec::new();
        }
        let mut children = vec![div().child("\u{a0}\u{a0}|\u{a0}\u{a0}").into_any_element()];
        let link = self.color("editorLink.activeForeground");
        let count = items.len();
        for (position, item) in items.into_iter().enumerate() {
            let line = item.start_line;
            let name = item.name.clone();
            let mut crumb = div()
                .id(gpui::SharedString::from(format!("diff-breadcrumb-{index}-{position}")))
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|crumb| crumb.text_color(link))
                .child(self.icon(crate::symbol_kinds::icon_file(item.kind), 16., 16., self.color(crate::symbol_kinds::color_key(item.kind))))
                .child("\u{a0}")
                .child(name)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        workbench.change_unchanged_regions(
                            |input| {
                                if let Some(region) = input.unchanged.iter_mut().find(|region| region.hidden_modified().contains(line)) {
                                    region.show_modified_line(line, RevealPreference::FromBottom);
                                }
                            },
                            cx,
                        );
                    }),
                );
            if position + 1 < count {
                crumb = crumb.child(self.icon("chevron-right", 16., 16., self.color("diffEditor.unchangedRegionForeground")));
            }
            children.push(crumb.into_any_element());
        }
        children
    }

    /// Window mouse listeners while a bar is dragged: each line height of movement shows or hides one line; a release
    /// without movement shows the reveal line count.
    fn unchanged_drag_listener(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workbench = cx.entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        moved.update(cx, |workbench, cx| workbench.drag_unchanged_bar(f32::from(event.position.y), cx));
                    }
                });
                let released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        released.update(cx, |workbench, cx| workbench.release_unchanged_bar(cx));
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    fn drag_unchanged_bar(&mut self, y: f32, cx: &mut Context<Self>) {
        let line_height = self.editors.viewport.line_height;
        let Some(drag) = self.editors.active_diff().and_then(|input| input.unchanged_drag) else { return };
        let delta = y - drag.start_y;
        let moved = drag.moved || delta.abs() > 2.;
        let line_delta = (delta / line_height).round() as isize;
        if drag.top {
            self.change_unchanged_regions(
                |input| {
                    let region = &mut input.unchanged[drag.region];
                    let max = region.line_count - region.visible_bottom;
                    region.visible_top = drag.start_count.saturating_add_signed(line_delta).min(max);
                    input.unchanged_drag = Some(UnchangedDrag { moved, ..drag });
                },
                cx,
            );
            return;
        }
        let Some(input) = self.editors.active_diff() else { return };
        let region = input.unchanged[drag.region];
        let count = drag.start_count.saturating_add_signed(-line_delta).min(region.line_count - region.visible_top);
        self.change_unchanged_regions(|input| input.unchanged_drag = Some(UnchangedDrag { moved, ..drag }), cx);
        if count != region.visible_bottom {
            self.set_visible_bottom(drag.region, count, cx);
        }
    }

    fn release_unchanged_bar(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.editors.active_diff().and_then(|input| input.unchanged_drag) else { return };
        self.change_unchanged_regions(|input| input.unchanged_drag = None, cx);
        if drag.moved {
            return;
        }
        let reveal = self.reveal_line_count();
        if drag.top {
            self.change_unchanged_regions(|input| input.unchanged[drag.region].show_more_above(reveal), cx);
            return;
        }
        let Some(region) = self.editors.active_diff().map(|input| input.unchanged[drag.region]) else { return };
        self.set_visible_bottom(drag.region, (region.visible_bottom + reveal).min(region.line_count - region.visible_top), cx);
    }

    /// The `diff-unchanged-lines` background of a line in an unchanged region.
    pub(super) fn unchanged_line_background(&self, input: &DiffInput, side: ZoneSide, line: usize) -> Option<Div> {
        input.unchanged_at(side, line)?;
        let color = self.theme.color("diffEditor.unchangedCodeBackground")?;
        Some(div().absolute().top_0().bottom_0().left_0().right_0().bg(color))
    }
}

/// A 1-based `[start, end)` line range as the editor's 0-based hidden lines.
fn zero_based(range: diff::LineRange) -> std::ops::Range<usize> {
    range.start - 1..range.end - 1
}
