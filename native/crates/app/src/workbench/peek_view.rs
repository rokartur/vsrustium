//! peekView.ts and zoneWidget.ts: the frame every peek draws over its view zone (the arrow, the head, the body
//! split into a preview and a tree), its two sashes, and the drawn preview of a file.

use gpui::{AnyElement, Context, Div, MouseButton, MouseDownEvent, PathBuilder, Stateful, StyledText, Task, Window, canvas, div, point, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::{OpenEditor, Position, Selection, TextDocument, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, char_column, expand_tabs, visible_column};

/// zoneWidget.ts: the arrow is a third of a line high; peekView.ts: the head is 1.2 lines.
pub(super) const ARROW_HEIGHT: f32 = 6.;
pub(super) const HEAD_HEIGHT: f32 = 22.;
pub(super) const UI_FONT: &str = ".SystemUIFont";

/// The peeks that can be open: the references peek and the call or type hierarchy peek.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PeekKind {
    References,
    Hierarchy,
}

/// zoneWidget.ts `_resizeSash` below the zone and the split view sash between preview and tree.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PeekSash {
    Zone,
    Split,
}

/// A peek's zone and layout: where it is, its height and split ratio, and its sashes' state.
pub(super) struct PeekLayout {
    pub(super) model_id: u32,
    pub(super) zone_id: u32,
    pub(super) height_in_lines: f32,
    pub(super) ratio: f32,
    /// The sash being dragged: which one, the pointer where the drag started, the size then and the widget's width.
    pub(super) sash_drag: Option<(PeekSash, f32, f32, f32)>,
    /// sash.ts: the hover border shows 300ms after the pointer enters a sash.
    hovered_sash: Option<PeekSash>,
    sash_hover: Option<Task<()>>,
}

/// The stored layout of a peek: its profile storage key, the height's key and its default.
pub(super) struct LayoutStorage {
    pub(super) key: &'static str,
    pub(super) height_key: &'static str,
    pub(super) default_height: f32,
}

impl Workbench {
    pub(super) fn peek_layout(&self, kind: PeekKind) -> Option<&PeekLayout> {
        match kind {
            PeekKind::References => self.references_peek_layout(),
            PeekKind::Hierarchy => self.hierarchy_peek_layout(),
        }
    }

    fn peek_layout_mut(&mut self, kind: PeekKind) -> Option<&mut PeekLayout> {
        match kind {
            PeekKind::References => self.references_peek_layout_mut(),
            PeekKind::Hierarchy => self.hierarchy_peek_layout_mut(),
        }
    }

    /// zoneWidget.ts `show`: a zone below `at` in the active editor, its stored height capped by
    /// `_getMaximumHeightInLines`, and the stored split ratio (0.7 without one).
    pub(super) fn open_peek_zone(&mut self, storage: &LayoutStorage, at: Position) -> PeekLayout {
        let layout: Value = self.profile_storage.lock().unwrap().get(storage.key).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_default();
        let ratio = layout["ratio"].as_f64().filter(|ratio| *ratio != 0.).unwrap_or(0.7) as f32;
        let stored_height = layout[storage.height_key].as_f64().filter(|height| *height != 0.).unwrap_or(storage.default_height as f64) as f32;
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("a peek opens in the active editor");
        let height_in_lines = stored_height.min(12f32.max(viewport.height / viewport.line_height * 0.8));
        let zone_id = editor.add_view_zone(Some(at), height_in_lines * viewport.line_height);
        PeekLayout { model_id: editor.model_id, zone_id, height_in_lines, ratio, sash_drag: None, hovered_sash: None, sash_hover: None }
    }

    /// The zone removed and, when `storage` is given, the layout stored.
    pub(super) fn close_peek_zone(&mut self, layout: &PeekLayout, storage: Option<&LayoutStorage>) {
        if let Some(storage) = storage {
            let stored = json!({ "ratio": layout.ratio, storage.height_key: layout.height_in_lines });
            self.profile_storage.lock().unwrap().set(storage.key, stored.to_string());
        }
        if let Some(editor) = self.editors.editors.iter_mut().chain(&mut self.editors.models).find(|editor| editor.model_id == layout.model_id) {
            editor.remove_view_zone(layout.zone_id);
        }
    }

    /// `_showImpl`: the range selected and its lines and the zone revealed.
    pub(super) fn reveal_peek_range(&mut self, start: Position, end: Position) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("a peek opens in the active editor");
        editor.set_selections(vec![Selection { anchor: start, head: end }]);
        let last = editor.document.lines.len() - 1;
        match end.line == last {
            true => editor.reveal_range(Position::new(last, 0), Position::new(last, 0), VerticalReveal::NearTop, viewport),
            false => editor.reveal_range(Position::new(start.line, 0), Position::new(end.line + 1, 0), VerticalReveal::Simple, viewport),
        }
        self.host.send_selection(editor, "api");
    }

    /// The zone's position (`ZoneWidget.position`).
    pub(super) fn peek_zone_position(&self, layout: &PeekLayout) -> Option<Position> {
        let editor = self.editors.editors.iter().find(|editor| editor.model_id == layout.model_id)?;
        editor.view_zone(layout.zone_id)?.anchor
    }

    /// peekView.ts `_doLayout`: the container less the head and the body's 1px top border.
    pub(super) fn peek_body_height(&self, kind: PeekKind) -> f32 {
        let Some(layout) = self.peek_layout(kind) else { return 0. };
        layout.height_in_lines * self.editors.viewport.line_height - 2. * ARROW_HEIGHT - (HEAD_HEIGHT + 1.)
    }

    /// The peek over its zone: the arrow, then the frame with `head` and the body `body` draws at the preview's width
    /// and the body height, with the split sash when it answers `true` (a preview and a tree, not a message).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn peek_zone(&self, kind: PeekKind, editor: &OpenEditor, content_left: f32, char_width: f32, head: Div, body: impl FnOnce(f32, f32, &mut Context<Self>) -> (AnyElement, bool), cx: &mut Context<Self>) -> Option<AnyElement> {
        let layout = self.peek_layout(kind).filter(|layout| layout.model_id == editor.model_id)?;
        let viewport = self.editors.viewport;
        let zone = editor.view_zone(layout.zone_id)?;
        let top = editor.view_zone_top(layout.zone_id, viewport)? - editor.scroll.top;
        let width = content_left + viewport.width - VERTICAL_SCROLLBAR_WIDTH;
        let frame = self.color("peekView.border");
        // zoneWidget.ts `show`: the arrow is not pretty at column 1 and moves to column 2.
        let anchor = zone.anchor?;
        let line = &editor.document.lines[anchor.line];
        let arrow_column = anchor.column.max(1).min(line.chars().count().max(1));
        let arrow_x = content_left + visible_column(line, arrow_column, editor.document.indentation.tab_size) as f32 * char_width - editor.scroll.left;
        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let mut path = PathBuilder::fill();
                path.move_to(point(x, y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(2. * ARROW_HEIGHT), y + px(ARROW_HEIGHT)));
                path.line_to(point(x + px(ARROW_HEIGHT), y));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, frame);
                }
            },
        )
        .absolute()
        .top_0()
        .left(px(arrow_x - ARROW_HEIGHT))
        .w(px(2. * ARROW_HEIGHT))
        .h(px(ARROW_HEIGHT));
        let container_height = zone.height - 2. * ARROW_HEIGHT;
        let body_height = container_height - HEAD_HEIGHT - 1.;
        let preview_width = ((width * layout.ratio).max(200.)).min(width - 100.);
        let (body, split) = body(preview_width, body_height, cx);
        let split_sash = split.then(|| self.peek_sash(kind, layout, PeekSash::Split, [preview_width - 2., 0., 4., body_height], (preview_width, width), cx));
        let container = div()
            .absolute()
            .top(px(ARROW_HEIGHT))
            .left_0()
            .right_0()
            .h(px(container_height + 2.))
            .overflow_hidden()
            .border_t_1()
            .border_b_1()
            .border_color(frame)
            .child(head)
            .child(
                div()
                    .h(px(body_height + 1.))
                    .border_t_1()
                    .border_color(frame)
                    .child(div().size_full().relative().flex().child(body).children(split_sash)),
            );
        let widget = div()
            .id(match kind {
                PeekKind::References => "references-peek",
                PeekKind::Hierarchy => "hierarchy-peek",
            })
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(width))
            .h(px(zone.height))
            .cursor_default()
            .occlude()
            .child(container)
            .child(arrow)
            // zoneWidget.ts `getHorizontalSashTop`: half the arrows' height above the zone's end.
            .child(self.peek_sash(kind, layout, PeekSash::Zone, [0., zone.height - ARROW_HEIGHT - 2., width, 4.], (layout.height_in_lines, width), cx))
            .children(layout.sash_drag.is_some().then(|| self.peek_sash_drag(kind, cx)));
        Some(widget.into_any_element())
    }

    /// The body's message (`.peekview-message`, "No results").
    pub(super) fn peek_message(&self, message: String) -> AnyElement {
        div().size_full().pt(px(39.)).flex().justify_center().text_size(px(13.)).font_family(UI_FONT).child(message).into_any_element()
    }

    /// The head: the title, its description and meta, and the actions on the right.
    pub(super) fn peek_head(&self, title: Stateful<Div>, actions: Vec<AnyElement>) -> Div {
        div()
            .h(px(HEAD_HEIGHT))
            .line_height(px(HEAD_HEIGHT))
            .flex()
            .justify_between()
            .flex_nowrap()
            .font_family(UI_FONT)
            .bg(self.color("peekViewTitle.background"))
            .child(title.ml(px(20.)).min_w_0().overflow_hidden().flex().items_baseline().text_size(px(13.)))
            .child(div().flex_1().flex().justify_end().items_center().pr(px(2.)).children(actions))
    }

    /// `setTitle(label, description)` and `setMetaTitle`.
    pub(super) fn peek_title_parts(&self, title: Stateful<Div>, label: String, description: String, meta: String) -> Stateful<Div> {
        let description_color = self.color("peekViewTitleDescription.foreground");
        title
            .child(div().whitespace_nowrap().text_color(self.color("peekViewTitleLabel.foreground")).child(label))
            .when(!description.is_empty(), |title| title.child(div().whitespace_nowrap().ml(px(0.5 * 0.9 * 13.)).text_size(px(0.9 * 13.)).text_color(description_color).child(description)))
            .when(!meta.is_empty(), |title| title.child(div().whitespace_nowrap().text_color(description_color).child(format!("\u{2009}-\u{2009}{meta}"))))
    }

    /// A `.monaco-sash` (sash.css): 4px, the mac resize cursor, `sash.hoverBorder` after the hover delay and while dragged.
    fn peek_sash(&self, kind: PeekKind, layout: &PeekLayout, sash: PeekSash, [x, y, w, h]: [f32; 4], (size, width): (f32, f32), cx: &mut Context<Self>) -> AnyElement {
        let active = layout.hovered_sash == Some(sash) || layout.sash_drag.is_some_and(|(dragged, ..)| dragged == sash);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let cursor = match sash {
            PeekSash::Zone => gpui::CursorStyle::ResizeRow,
            PeekSash::Split => gpui::CursorStyle::ResizeColumn,
        };
        div()
            .id(match sash {
                PeekSash::Zone => "peek-zone-sash",
                PeekSash::Split => "peek-split-sash",
            })
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .cursor(cursor)
            .when(active, |element| element.bg(hover_border))
            .on_hover(cx.listener(move |workbench, hovered: &bool, window, cx| {
                let Some(layout) = workbench.peek_layout_mut(kind) else { return };
                layout.hovered_sash = None;
                layout.sash_hover = hovered.then(|| {
                    cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(super::part_grid::SASH_HOVER_DELAY).await;
                        let _ = workbench.update(cx, |workbench, cx| {
                            if let Some(layout) = workbench.peek_layout_mut(kind) {
                                layout.hovered_sash = Some(sash);
                                cx.notify();
                            }
                        });
                    })
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(layout) = workbench.peek_layout_mut(kind) else { return };
                    let pointer = match sash {
                        PeekSash::Zone => f32::from(event.position.y),
                        PeekSash::Split => f32::from(event.position.x),
                    };
                    layout.sash_drag = Some((sash, pointer, size, width));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a peek sash drags.
    fn peek_sash_drag(&self, kind: PeekKind, cx: &mut Context<Self>) -> AnyElement {
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| workbench.drag_peek_sash(kind, f32::from(event.position.x), f32::from(event.position.y), cx));
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &gpui::MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        if let Some(layout) = workbench.peek_layout_mut(kind) {
                            layout.sash_drag = None;
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_0()
        .into_any_element()
    }

    /// zoneWidget.ts `_initSash` `onDidChange`: whole lines between 5 and 35 exclusive; the split view keeps the
    /// preview at least 200px and the tree at least 100px wide, and the ratio follows (`onDidSashChange`).
    pub(super) fn drag_peek_sash(&mut self, kind: PeekKind, x: f32, y: f32, cx: &mut Context<Self>) {
        let line_height = self.editors.viewport.line_height;
        let Some(layout) = self.peek_layout_mut(kind) else { return };
        let Some((sash, start, size, width)) = layout.sash_drag else { return };
        match sash {
            PeekSash::Zone => {
                let delta = ((y - start) / line_height).trunc();
                let height_in_lines = size + delta;
                if height_in_lines <= 5. || height_in_lines >= 35. || height_in_lines == layout.height_in_lines {
                    return;
                }
                layout.height_in_lines = height_in_lines;
                let (model_id, zone_id) = (layout.model_id, layout.zone_id);
                let editor = self.editors.editors.iter_mut().find(|editor| editor.model_id == model_id).expect("the peek's editor is open");
                editor.view_zones.iter_mut().find(|zone| zone.id == zone_id).expect("the peek's zone").height = height_in_lines * line_height;
            }
            PeekSash::Split => {
                let preview_width = (size + x - start).max(200.).min(width - 100.);
                layout.ratio = preview_width / width;
            }
        }
        cx.notify();
    }

    /// The file a peek previews: an open model's, else one the peek read.
    pub(super) fn peek_document<'a>(&'a self, uri: &str, read: &'a [TextDocument]) -> Option<&'a TextDocument> {
        let open = self.editors.editors.iter().chain(&self.editors.models).map(|editor| &editor.document);
        open.chain(read).find(|document| document.uri_string() == uri)
    }

    /// `createModelReference`: a file no editor has open read for the preview into `read`.
    pub(super) fn read_peek_document(&self, uri: &str, resource: &Value, read: &mut Vec<TextDocument>) {
        if self.peek_document(uri, read).is_some() {
            return;
        }
        if let Ok((_, path)) = super::host_requests::uri_parts(resource)
            && let Ok(document) = TextDocument::read(&path, &self.languages, &self.settings.lock().unwrap())
        {
            read.push(document);
        }
    }

    /// `revealRangeInCenter` of `line` without scrolling past the last line.
    pub(super) fn peek_preview_scroll(&self, line: usize, line_count: usize, body_height: f32) -> f32 {
        let line_height = self.editors.viewport.line_height;
        let max = (line_count as f32 * line_height - body_height).max(0.);
        (line as f32 * line_height + line_height / 2. - body_height / 2.).clamp(0., max)
    }

    /// The preview's lines with their syntax colors, the line numbers in the gutter, `marks` (0-based line and
    /// column ranges) drawn as matches and `cursor_line` outlined.
    // shortcut: drawn lines, not an embedded editor; it scrolls with the wheel but cannot be edited or focused and has
    // no indent guides, folding or scrollbars. A second editor view replaces it once the editor groups row lands.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn peek_preview_lines(&self, id: &'static str, document: Option<&TextDocument>, marks: &[(usize, usize, usize, usize)], cursor_line: usize, scroll: f32, width: f32, height: f32, char_width: f32, window: &Window) -> Stateful<Div> {
        let background = self.color("peekViewEditor.background");
        let preview = div().id(id).w(px(width)).h(px(height)).flex_none().overflow_hidden().relative().bg(background);
        let Some(document) = document else { return preview };
        let line_height = self.editors.viewport.line_height;
        let lines = &document.lines;
        let tab_size = document.indentation.tab_size;
        let content_left = self.preview_content_left(lines.len(), window);
        let first = (scroll / line_height).floor() as usize;
        let last = (((scroll + height) / line_height).ceil() as usize).min(lines.len());
        let match_background = self.theme.color("peekViewEditor.matchHighlightBackground");
        let match_border = self.theme.color("peekViewEditor.matchHighlightBorder");
        let line_border = self.color("editor.lineHighlightBorder");
        let mut rows = Vec::new();
        for index in first..last {
            let text = &lines[index];
            let mut content = div().absolute().top_0().h_full().left(px(content_left)).right_0();
            for &(start_line, start, end_line, end) in marks.iter().filter(|range| range.0 <= index && index <= range.2) {
                let from = if start_line == index { char_column(text, start) } else { 0 };
                let to = if end_line == index { char_column(text, end) } else { text.chars().count() };
                let left = visible_column(text, from, tab_size) as f32 * char_width;
                let right = visible_column(text, to, tab_size) as f32 * char_width;
                content = content.child(
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .left(px(left))
                        .w(px(right - left))
                        .when_some(match_background, |mark, color| mark.bg(color))
                        .when_some(match_border, |mark, color| mark.border_2().border_color(color)),
                );
            }
            let highlights = document.tokens.highlights(&self.theme, &self.languages, lines, index, tab_size);
            content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(highlights)));
            let number_color = self.color(if index == cursor_line { "editorLineNumber.activeForeground" } else { "editorLineNumber.foreground" });
            let number = div().absolute().top_0().left_0().w(px(content_left - 26. + line_height)).flex().justify_end().text_color(number_color).child((index + 1).to_string());
            rows.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(index as f32 * line_height - scroll))
                    .h(px(line_height))
                    .when(index == cursor_line, |row| row.child(div().absolute().top_0().bottom_0().left(px(content_left)).right_0().border_2().border_color(line_border)))
                    .child(number)
                    .child(content),
            );
        }
        preview
            .font_family(super::editor_view::EDITOR_FONT)
            .text_size(px(super::editor_view::EDITOR_FONT_SIZE * self.editor_zoom.multiplier()))
            .line_height(px(line_height))
            .text_color(self.color("editor.foreground"))
            .child(div().absolute().top_0().left_0().h_full().w(px(content_left - 26. + line_height + 26.)).bg(self.color("peekViewEditorGutter.background")))
            .children(rows)
    }

    /// The embedded editor's gutter takes the editor's options: the glyph margin, the line numbers and the folding.
    fn preview_content_left(&self, line_count: usize, window: &Window) -> f32 {
        let metrics = super::editor_view::EditorMetrics::measure(window, line_count, super::editor_view::LineNumbers::On, self.editor_zoom.multiplier());
        metrics.content_left()
    }
}
