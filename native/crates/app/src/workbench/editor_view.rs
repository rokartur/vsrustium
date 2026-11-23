//! The editor part with open files: the connected tab strip (connectedEditorTabs.css), the breadcrumbs,
//! and the code editor's view parts (margin, current line, word highlights, indent guides, text, cursor,
//! decorationsOverviewRuler.ts). Sizes are the reference's CSS pixels with editor.fontSize 12 and lineHeight 18.

use std::ops::Range;

use gpui::{
    Bounds, Context, Div, MouseButton, MouseDownEvent, Rgba, Window, canvas, div, fill, font, point, prelude::*, px, size,
    uniform_list,
};

use super::Workbench;
use crate::editor::{OpenEditor, column_at_visible, expand_tabs, visible_column};
use crate::theme::ThemeKind;

const LINE_HEIGHT: f32 = 18.;
const EDITOR_FONT: &str = "Menlo";
const EDITOR_FONT_SIZE: f32 = 12.;
/// The glyph margin, then lineDecorationsWidth 10 plus 16 for the folding controls (editorOptions.ts layout).
const GLYPH_MARGIN_WIDTH: f32 = 18.;
const DECORATIONS_WIDTH: f32 = 26.;
/// The workbench text editor sets lineNumbersMinChars to 3 (textEditor.ts).
const LINE_NUMBERS_MIN_CHARS: usize = 3;
const OVERVIEW_RULER_WIDTH: f32 = 14.;
const TAB_STRIP_HEIGHT: f32 = 32.;
const BREADCRUMBS_HEIGHT: f32 = 22.;

impl Workbench {
    pub(super) fn text_editor_part(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let editor = self.editors.active().expect("text editor part is drawn only with an editor open");
        div()
            .flex_1()
            .relative()
            .flex()
            .flex_col()
            .border_1()
            .rounded(px(8.))
            .bg(self.color("editor.background"))
            .border_color(self.color("surface.border"))
            .child(self.tab_strip(cx))
            .child(self.breadcrumbs(editor))
            .child(self.text_area(editor, window, cx))
            // The active tab reaches into the part's top border, which Chrome still draws over it.
            .child(div().absolute().inset(px(-1.)).border_1().rounded(px(8.)).border_color(self.color("surface.border")))
    }

    /// One strip behind every tab; the active tab is an editor-colored fill whose bottom shoulders curve
    /// into the strip, so the active tab and the editor below read as one surface.
    fn tab_strip(&self, cx: &mut Context<Self>) -> Div {
        let active = self.editors.active_index().expect("tab strip is drawn only with an editor open");
        let icon_color = self.color("icon.foreground");
        let actions = div()
            .ml_auto()
            // Centered above the strip's bottom border, as the tabs are.
            .h(px(TAB_STRIP_HEIGHT - 1.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(self.icon("split-horizontal", 22., 16., icon_color))
            .child(self.icon("ellipsis", 22., 16., icon_color));
        // The line under the strip (tabs-border-bottom) runs below the shoulders; the active fill covers it.
        let bottom_line = div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(self.connected_tab_border());
        div()
            .relative()
            .h(px(TAB_STRIP_HEIGHT))
            .flex_none()
            .flex()
            .bg(self.tab_strip_background())
            .child(bottom_line)
            .children((0..self.editors.editors.len()).map(|index| self.tab(index, index == active, cx)))
            .child(actions)
    }

    /// `--modern-ui-connected-tab-strip-background` (connectedEditorTabs.ts): the connected tabs background,
    /// else the tabs background, else the editor's.
    fn tab_strip_background(&self) -> Rgba {
        ["editorGroupHeader.connectedTabsBackground", "editorGroupHeader.tabsBackground", "editor.background"]
            .into_iter()
            .find_map(|key| self.theme.color(key))
            .expect("editor.background has a registry default")
    }

    /// `--modern-ui-connected-tab-border`: the header's tabs border; in high contrast the focus border of the active group.
    fn connected_tab_border(&self) -> Rgba {
        match self.theme.kind {
            ThemeKind::HighContrastDark => self.color("focusBorder"),
            _ => ["editorGroupHeader.tabsBorder", "tab.border", "editor.background"]
                .into_iter()
                .find_map(|key| self.theme.color(key))
                .expect("editor.background has a registry default"),
        }
    }

    /// A tab sized to its label (tab.sizing fit): 8px before the label, 30.5px after it for the close action.
    fn tab(&self, index: usize, active: bool, cx: &mut Context<Self>) -> Div {
        let editor = &self.editors.editors[index];
        let foreground = if active { self.foreground_or("modernTab.activeForeground") } else { self.foreground_or("tab.inactiveForeground") };
        let label = div()
            .mt(px(3.5))
            .h(px(24.))
            .line_height(px(24.))
            .whitespace_nowrap()
            .when(editor.preview, |label| label.italic())
            .child(editor.document.name());
        let close = self.icon("close-small", 20., 16., foreground).id(("close", index)).rounded(px(4.)).on_click(cx.listener(move |workbench, _, _, cx| {
            cx.stop_propagation();
            workbench.editors.close(index);
            workbench.editors_changed(cx);
        }));
        let tab = div()
            .id(("tab", index))
            .relative()
            .h_full()
            .flex_none()
            .flex()
            .pl(px(8.))
            .pr(px(30.5))
            .text_color(foreground)
            .child(label)
            .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, _, cx| {
                workbench.editors.activate(index);
                if event.click_count() == 2 {
                    workbench.editors.pin(index);
                }
                workbench.editors_changed(cx);
            }))
            .when(active, |tab| tab.child(div().absolute().top(px(5.5)).right(px(6.5)).child(close)));
        if !active {
            return div().h_full().flex_none().child(tab).mr(px(8.));
        }
        // The fill starts in the part's top border row; its caps and shoulders are cornerRadius-medium plus the stroke.
        let surface = self.color("editor.background");
        let border = self.connected_tab_border();
        let radius = px(7.);
        let shoulder = |piece: Div| piece.absolute().bottom_0().size(radius).bg(surface);
        let strip_corner = || div().size_full().bg(self.tab_strip_background()).border_b_1().border_color(border);
        div()
            .h_full()
            .flex_none()
            .relative()
            .child(div().absolute().top(px(-1.)).left_0().right_0().h(px(TAB_STRIP_HEIGHT + 1.)).bg(surface).border_t_1().border_r_1().rounded_t(radius).border_color(border))
            .when_some(self.theme.color("tab.activeBorder"), |tab, color| tab.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
            // The first tab meets the part's own rounded corner, so it has no left shoulder.
            .when(index > 0, |tab| tab.child(shoulder(div().left(-radius)).child(strip_corner().border_r_1().rounded_br(radius))))
            .child(shoulder(div().left_full()).child(strip_corner().border_l_1().rounded_bl(radius)))
            .child(tab)
    }

    /// The file's path below the workspace folder, one item per segment (breadcrumbs.filePath relative).
    fn breadcrumbs(&self, editor: &OpenEditor) -> Div {
        let path = &editor.document.path;
        let relative = self.folder.as_ref().and_then(|tree| path.strip_prefix(&tree.root).ok()).unwrap_or(path);
        let segments: Vec<String> = relative.iter().map(|segment| segment.to_string_lossy().into_owned()).collect();
        let foreground = self.color("breadcrumb.foreground");
        let last = segments.len() - 1;
        let items = segments.into_iter().enumerate().map(|(index, segment)| {
            // shortcut: no file icon theme, so the 16px icon slot stays empty; draw the icon once Seti is ported.
            div()
                .flex()
                .items_center()
                .pl(px(16.))
                .pr(px(8.))
                .whitespace_nowrap()
                .child(segment)
                .when(index < last, |item| item.child(self.icon("chevron-right", 16., 16., foreground).ml(px(8.))))
        });
        // editorGroupView.ts: the title's bottom border, drawn over the breadcrumbs' last row.
        let title_border = ["editorGroupHeader.border", "contrastBorder"].into_iter().find_map(|key| self.theme.color(key));
        div()
            .relative()
            .h(px(BREADCRUMBS_HEIGHT))
            .flex_none()
            .flex()
            .bg(self.color("breadcrumb.background"))
            .text_color(foreground)
            .children(items)
            .when_some(title_border, |crumbs, color| crumbs.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
    }

    fn text_area(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let metrics = EditorMetrics::measure(window, editor.document.lines.len());
        let area_left = self.text_area_left.clone();
        let list = uniform_list("editor-lines", editor.document.lines.len(), cx.processor(move |workbench, range: Range<usize>, _, cx| workbench.editor_lines(range, metrics, cx)))
            .track_scroll(&self.editor_scroll)
            .size_full();
        div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .font_family(EDITOR_FONT)
            .text_size(px(EDITOR_FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .child(canvas(move |bounds, _, _| area_left.set(f32::from(bounds.left())), |_, _, _, _| {}).absolute().inset_0())
            .child(list)
            .child(self.overview_ruler(editor).absolute().top_0().right_0().h_full())
    }

    fn editor_lines(&mut self, range: Range<usize>, metrics: EditorMetrics, cx: &mut Context<Self>) -> Vec<Div> {
        let editor = self.editors.active().expect("lines are drawn only with an editor open");
        let tab_size = editor.document.indentation.tab_size;
        let (cursor_line, cursor_column) = editor.cursor;
        let colors = LineColors {
            number: self.color("editorLineNumber.foreground"),
            active_number: self.color("editorLineNumber.activeForeground"),
            text: self.color("editor.foreground"),
            line_border: self.color("editor.lineHighlightBorder"),
            word: self.theme.color("editor.wordHighlightTextBackground"),
            word_border: self.theme.color("editor.wordHighlightTextBorder"),
            guide: self.color("editorIndentGuide.background1"),
            cursor: self.color("editorCursor.foreground"),
        };
        // currentLineHighlight.css: high contrast draws the exact border 1px wide.
        let line_border_width = if self.theme.kind == ThemeKind::HighContrastDark { 1. } else { 2. };
        let char_width = metrics.char_width;
        range
            .map(|index| {
                let text = &editor.document.lines[index];
                let is_cursor_line = index == cursor_line;
                let number = div()
                    .w(px(metrics.line_numbers_width))
                    .flex_none()
                    .text_right()
                    .text_color(if is_cursor_line { colors.active_number } else { colors.number })
                    .child((index + 1).to_string());
                let mut content = div().relative().flex_1().h_full().text_color(colors.text);
                if is_cursor_line {
                    content = content.child(div().absolute().inset_0().border(px(line_border_width)).border_color(colors.line_border));
                }
                for level in 0..editor.guides[index] {
                    let x = (level * editor.document.indentation.tab_size) as f32 * char_width;
                    content = content.child(div().absolute().top_0().left(px(x)).w(px(1.)).h_full().bg(colors.guide));
                }
                for (_, columns) in editor.highlights.iter().filter(|(line, _)| *line == index) {
                    let start = visible_column(text, columns.start, tab_size) as f32 * char_width;
                    let end = visible_column(text, columns.end, tab_size) as f32 * char_width;
                    let mut highlight = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = colors.word {
                        highlight = highlight.bg(background);
                    }
                    // shortcut: GPUI has no dotted border, so the high contrast word highlight is dashed.
                    if let Some(border) = colors.word_border {
                        highlight = highlight.border_1().border_dashed().border_color(border);
                    }
                    content = content.child(highlight);
                }
                content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(expand_tabs(text, tab_size)));
                if is_cursor_line {
                    // shortcut: the cursor does not blink (editor.cursorBlinking "blink" hides it every 500ms).
                    let x = visible_column(text, cursor_column, tab_size) as f32 * char_width;
                    content = content.child(div().absolute().top_0().left(px(x)).w(px(2.)).h_full().bg(colors.cursor));
                }
                let content = content.on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    let x = f32::from(event.position.x) - workbench.text_area_left.get() - metrics.content_left();
                    workbench.click_text(index, x / char_width, cx);
                }));
                div()
                    .w_full()
                    .h(px(LINE_HEIGHT))
                    .flex()
                    .child(div().w(px(GLYPH_MARGIN_WIDTH)).flex_none())
                    .child(number)
                    .child(div().w(px(DECORATIONS_WIDTH)).flex_none())
                    .child(content)
            })
            .collect()
    }

    fn click_text(&mut self, line: usize, visible: f32, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let tab_size = editor.document.indentation.tab_size;
        let column = column_at_visible(&editor.document.lines[line], visible, tab_size);
        editor.set_cursor(line, column);
        self.host.send_selection(editor, "mouse");
        cx.notify();
    }

    /// decorationsOverviewRuler.ts `_render`, in device pixels: word highlights in the center lane, the cursor
    /// across all lanes, then the 1px border on the left and top edges.
    fn overview_ruler(&self, editor: &OpenEditor) -> gpui::Canvas<()> {
        let line_count = editor.document.lines.len();
        let cursor_line = editor.cursor.0;
        let highlight_lines: Vec<usize> = editor.highlights.iter().map(|(line, _)| *line).collect();
        let highlight = self.color("editorOverviewRuler.wordHighlightTextForeground");
        let cursor = self.theme.color("editorCursor.foreground").map(|color| Rgba { a: color.a * 0.7, ..color });
        let border = self.theme.color("editorOverviewRuler.border");
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let scale = window.scale_factor();
                let device = |value: f32| px(value / scale);
                let origin = bounds.origin;
                let paint = |window: &mut Window, x: f32, y: f32, width: f32, height: f32, color: Rgba| {
                    window.paint_quad(fill(Bounds::new(origin + point(device(x), device(y)), size(device(width), device(height))), color));
                };
                let canvas_width = (OVERVIEW_RULER_WIDTH * scale).floor();
                let canvas_height = (f32::from(bounds.size.height) * scale).floor();
                let viewport_height = f32::from(bounds.size.height);
                // scrollBeyondLastLine: the last line can scroll to the top of the viewport.
                let scroll_height = line_count as f32 * LINE_HEIGHT + viewport_height - LINE_HEIGHT;
                let ratio = canvas_height / scroll_height;
                let lanes = RulerLanes::new(canvas_width);
                let min_height = (6. * scale).floor();
                let mut previous: Option<(f32, f32)> = None;
                for line in &highlight_lines {
                    let mut y1 = (*line as f32 * LINE_HEIGHT * ratio).floor();
                    let mut y2 = ((*line as f32 * LINE_HEIGHT + LINE_HEIGHT) * ratio).floor();
                    if y2 - y1 < min_height {
                        let half = (min_height / 2.).floor();
                        let center = ((y1 + y2) / 2.).floor().clamp(half, canvas_height - half);
                        (y1, y2) = (center - half, center + half);
                    }
                    previous = match previous {
                        Some((start, end)) if y1 <= end + 1. => Some((start, end.max(y2))),
                        Some((start, end)) => {
                            paint(window, lanes.center_x, start, lanes.center_width, end - start, highlight);
                            Some((y1, y2))
                        }
                        None => Some((y1, y2)),
                    };
                }
                if let Some((start, end)) = previous {
                    paint(window, lanes.center_x, start, lanes.center_width, end - start, highlight);
                }
                if let Some(cursor) = cursor {
                    let height = (2. * scale).floor();
                    let half = (height / 2.).floor();
                    let center = (cursor_line as f32 * LINE_HEIGHT * ratio).floor().clamp(half, canvas_height - half);
                    paint(window, lanes.full_x, center - half, lanes.full_width, height, cursor);
                }
                // A 1px canvas stroke along x = 0 and y = 0 covers half of the first device column and row.
                if let Some(border) = border {
                    let half_covered = Rgba { a: border.a * 0.5, ..border };
                    paint(window, 0., 0., 1., canvas_height, half_covered);
                    paint(window, 1., 0., canvas_width - 1., 1., half_covered);
                }
            },
        )
        .w(px(OVERVIEW_RULER_WIDTH))
    }

    /// The selection, indentation, encoding, end of line and language items (editorStatus.ts), right to left
    /// before the notifications bell.
    pub(super) fn editor_status_items(&self, foreground: Rgba) -> Option<Div> {
        let editor = self.editors.active()?;
        let document = &editor.document;
        let (line, column) = editor.cursor;
        let visible = visible_column(&document.lines[line], column, document.indentation.tab_size);
        let indentation = match document.indentation.insert_spaces {
            true => format!("Spaces: {}", document.indentation.tab_size),
            false => format!("Tab Size: {}", document.indentation.tab_size),
        };
        let encoding = if document.encoding == "utf8bom" { "UTF-8 with BOM" } else { "UTF-8" };
        let eol = if document.eol == "\r\n" { "CRLF" } else { "LF" };
        let labels = [format!("Ln {}, Col {}", line + 1, visible + 1), indentation, encoding.into(), eol.into(), self.languages.name(&document.language_id).to_string()];
        let items = labels.into_iter().map(|label| div().h_full().mx(px(3.)).px(px(5.)).flex().items_center().whitespace_nowrap().child(label));
        Some(div().h_full().flex().text_color(foreground).children(items))
    }
}

#[derive(Clone, Copy)]
struct EditorMetrics {
    char_width: f32,
    line_numbers_width: f32,
}

impl EditorMetrics {
    fn content_left(&self) -> f32 {
        GLYPH_MARGIN_WIDTH + self.line_numbers_width + DECORATIONS_WIDTH
    }

    /// `typicalHalfwidthCharacterWidth` and `maxDigitWidth` of the editor font; the line numbers are as wide
    /// as the longest number, at least lineNumbersMinChars digits, rounded (editorOptions.ts).
    fn measure(window: &Window, line_count: usize) -> Self {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font(EDITOR_FONT));
        let font_size = px(EDITOR_FONT_SIZE);
        let advance = |c: char| f32::from(text_system.advance(font_id, font_size, c).expect("Menlo has ASCII glyphs").width);
        let max_digit_width = ('0'..='9').map(advance).fold(0., f32::max);
        let digits = line_count.to_string().len().max(LINE_NUMBERS_MIN_CHARS);
        EditorMetrics { char_width: advance('x'), line_numbers_width: (digits as f32 * max_digit_width).round() }
    }
}

struct LineColors {
    number: Rgba,
    active_number: Rgba,
    text: Rgba,
    line_border: Rgba,
    word: Option<Rgba>,
    word_border: Option<Rgba>,
    guide: Rgba,
    cursor: Rgba,
}

/// `_initLanes` with three lanes: one device pixel for the border, then three thirds, the center taking the rest.
struct RulerLanes {
    center_x: f32,
    center_width: f32,
    full_x: f32,
    full_width: f32,
}

impl RulerLanes {
    fn new(canvas_width: f32) -> Self {
        let remaining = canvas_width - 1.;
        let side = (remaining / 3.).floor();
        RulerLanes { center_x: 1. + side, center_width: remaining - 2. * side, full_x: 1., full_width: remaining }
    }
}


