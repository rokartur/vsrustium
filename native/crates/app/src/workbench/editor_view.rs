//! The editor part with open files: the connected tab strip (connectedEditorTabs.css), the breadcrumbs,
//! and the code editor's view parts (margin, current line, word highlights, indent guides, text, cursor,
//! decorationsOverviewRuler.ts). Sizes are the reference's CSS pixels with editor.fontSize 12 and lineHeight 18.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use std::time::{Duration, Instant};

use gpui::{
    Axis, Bounds, Context, DispatchPhase, Div, ElementInputHandler, FontFeatures, Hitbox, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    PathBuilder, Rgba, ScrollDelta, ScrollWheelEvent, Stateful, StyledText, Task, Window, canvas, div, fill, font, point, prelude::*, px, rgb, size,
};

pub(super) mod drag_and_drop;
pub(super) mod sticky_scroll_view;

use files::encoding::SUPPORTED_ENCODINGS;
use serde_json::json;

use super::Workbench;
use super::minimap_view::minimap_info;
use crate::brackets::BracketRange;
use crate::editor::minimap::MinimapLayoutInfo;
use crate::editor::{
    HORIZONTAL_SCROLLBAR_HEIGHT, Indentation, LINE_HEIGHT, OpenEditor, Position, Reveal, Scrollbar, Selection, VERTICAL_SCROLLBAR_WIDTH, VerticalReveal, Viewport, column_at_visible, commands,
    expand_tabs, line_overflow, show_more_width, visible_column, word_at,
};
use crate::editor::folding::FoldDecoration;
use crate::editor::whitespace::whitespace_marks;
use crate::theme::ThemeKind;

const EDITOR_FONT: &str = "Menlo";
/// `editor.fontSize` at zoom level 0.
const EDITOR_FONT_SIZE: f32 = 12.;
/// lineDecorationsWidth 10 plus 16 for the folding controls (editorOptions.ts layout).
const DECORATIONS_WIDTH: f32 = 26.;
/// The workbench text editor sets lineNumbersMinChars to 3 (textEditor.ts).
const LINE_NUMBERS_MIN_CHARS: usize = 3;
const OVERVIEW_RULER_WIDTH: f32 = 14.;
const TAB_STRIP_HEIGHT: f32 = 32.;
const BREADCRUMBS_HEIGHT: f32 = 22.;
/// scrollableElement.ts `HIDE_TIMEOUT`; scrollbars.css fades in over 100ms and out over 800ms.
const SCROLLBAR_HIDE_TIMEOUT: Duration = Duration::from_millis(500);
const SCROLLBAR_FADE_IN: Duration = Duration::from_millis(100);
const SCROLLBAR_FADE_OUT: Duration = Duration::from_millis(800);
const SCROLL_SHADOW_HEIGHT: f32 = 6.;
const FOLD_CONTROLS_FADE: Duration = Duration::from_millis(500);

impl Workbench {
    pub(super) fn text_editor_part(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let editor = self.editors.active().expect("text editor part is drawn only with an editor open");
        self.schedule_unusual_line_terminators_check(editor, window, cx);
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
            .children(self.unicode_banner(editor, cx))
            .child(self.text_area(editor, window, cx))
            // The active tab reaches into the part's top border, which Chrome still draws over it.
            .child(div().absolute().inset(px(-1.)).border_1().rounded(px(8.)).border_color(self.color("surface.border")))
    }

    /// One strip behind every tab; the active tab is an editor-colored fill whose bottom shoulders curve
    /// into the strip, so the active tab and the editor below read as one surface.
    fn tab_strip(&self, cx: &mut Context<Self>) -> Div {
        let active = self.editors.active_index().expect("tab strip is drawn only with an editor open");
        let icon_color = self.color("icon.foreground");
        // multieditortabscontrol.css: .editor-actions is cursor default, its action items pointer (actionbar.css).
        let actions = div()
            .ml_auto()
            .cursor_default()
            // Centered above the strip's bottom border, as the tabs are.
            .h(px(TAB_STRIP_HEIGHT - 1.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(self.icon("split-horizontal", 22., 16., icon_color).cursor_pointer())
            .child(self.icon("ellipsis", 22., 16., icon_color).cursor_pointer());
        // The line under the strip (tabs-border-bottom) runs below the shoulders; the active fill covers it.
        let bottom_line = div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(self.connected_tab_border());
        // editortabscontrol.css: the whole title, tabs and their close buttons included, is cursor pointer.
        div()
            .relative()
            .h(px(TAB_STRIP_HEIGHT))
            .flex_none()
            .cursor_pointer()
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
            ThemeKind::HighContrastDark | ThemeKind::HighContrastLight => self.color("focusBorder"),
            _ => ["editorGroupHeader.tabsBorder", "tab.border", "editor.background"]
                .into_iter()
                .find_map(|key| self.theme.color(key))
                .expect("editor.background has a registry default"),
        }
    }

    /// editorTabsControl.ts `getHoverTitle`: the long title, with a preview note on a preview editor's tab.
    fn tab_hover(&self, index: usize) -> impl IntoElement {
        let editor = &self.editors.editors[index];
        let title = crate::quick_input::path_label(&editor.document.path, None);
        let content = if editor.preview { super::hover::HoverContent::Preview(title) } else { super::hover::HoverContent::Text(title) };
        self.hover_target(("tab-hover", index), content, super::hover::HoverOptions::mouse())
    }

    /// A tab sized to its label (tab.sizing fit): 8px before the label, 30.5px after it for the close action.
    fn tab(&self, index: usize, active: bool, cx: &mut Context<Self>) -> Div {
        let editor = &self.editors.editors[index];
        let readonly = self.readonly(&editor.document.scheme, &editor.document.path).is_some();
        let action_foreground = if active { self.foreground_or("modernEditorTab.activeForeground") } else { self.foreground_or("tab.inactiveForeground") };
        // tabs.css colors `.tab-label:not(.monaco-decoration-itemColor)`: a decorated label (read-only, no color of its
        // own) inherits the tab's color from multieditortabscontrol.css, `tab.activeForeground` when active.
        let foreground = if active && readonly { self.foreground_or("tab.activeForeground") } else { action_foreground };
        let label = div()
            .mt(px(3.5))
            .h(px(24.))
            .line_height(px(24.))
            .whitespace_nowrap()
            .when(editor.preview, |label| label.italic())
            .child(editor.document.name());
        let file_icon = self.document_icon(editor, 24., foreground);
        // textFileService.ts decorations: a read-only file's label ends in the lockSmall badge at 0.75 opacity. The
        // iconlabel.css `::after` font-size 90% (11.7px) wins over the decoration rule's 16px, which loads first.
        let lock = readonly.then(|| self.icon("lock-small", 11.7, 11.7, foreground).mt(px(9.5)).ml(px(5.)).opacity(0.75));
        // A dirty tab shows a filled circle where the close action goes, also when inactive (tabstitlecontrol.css).
        let close_icon = if editor.is_dirty() { "circle-filled" } else { "close-small" };
        let close = self.icon(close_icon, 20., 16., action_foreground).id(("close", index)).rounded(px(4.)).on_click(cx.listener(move |workbench, _, _, cx| {
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
            .pr(px(30.5))
            .text_color(foreground)
            .map(|tab| match file_icon {
                Some(icon) => tab.pl(px(6.)).child(icon.mt(px(3.5))).child(label.ml(px(6.))),
                None => tab.pl(px(8.)).child(label),
            })
            .children(lock)
            .on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, _, cx| {
                workbench.editors.activate(index);
                if event.click_count() == 2 {
                    workbench.editors.pin(index);
                }
                workbench.editors_changed(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| workbench.show_tab_context_menu(index, event.position, window, cx)))
            .child(self.tab_hover(index))
            .when(active || editor.is_dirty(), |tab| tab.child(div().absolute().top(px(5.5)).right(px(6.5)).child(close)));
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

    fn document_icon(&self, editor: &OpenEditor, line_height: f32, foreground: Rgba) -> Option<Div> {
        let document = &editor.document;
        self.file_icons.as_ref()?.file_icon(&document.path, &document.language_id, self.theme.kind, 13., line_height, foreground)
    }

    /// The file's path below its workspace folder, one item per segment (breadcrumbs.filePath relative), after the
    /// folder's name in a workspace (`BreadcrumbsModel._initFilePathInfo`).
    fn breadcrumbs(&self, editor: &OpenEditor) -> Div {
        let path = &editor.document.path;
        let workspace = self.workspace();
        let folder = workspace.and_then(|workspace| workspace.folder_of(path));
        let relative: PathBuf = match folder {
            Some(folder) => path.iter().skip(folder.path.iter().count()).collect(),
            None => path.clone(),
        };
        let mut segments: Vec<String> = relative.iter().map(|segment| segment.to_string_lossy().into_owned()).collect();
        if let Some(folder) = folder.filter(|_| workspace.is_some_and(|workspace| workspace.file.is_some())) {
            segments.insert(0, folder.name.clone());
        }
        let foreground = self.color("breadcrumb.foreground");
        let last = segments.len() - 1;
        let items = segments.into_iter().enumerate().map(|(index, segment)| {
            // The 16px before each item is the item's own ::before; folders have no icon in Seti, the file does.
            let file_icon = if index == last { self.document_icon(editor, BREADCRUMBS_HEIGHT, foreground) } else { None };
            div()
                .flex()
                .items_center()
                .pl(px(16.))
                .pr(px(8.))
                .whitespace_nowrap()
                .when_some(file_icon, |item, icon| item.child(icon.mr(px(6.))))
                .child(segment)
                .when(index < last, |item| item.child(self.icon("chevron-right", 16., 16., foreground).ml(px(8.))))
        });
        // editorGroupView.ts: the title's bottom border, drawn over the breadcrumbs' last row.
        let title_border = ["editorGroupHeader.border", "contrastBorder"].into_iter().find_map(|key| self.theme.color(key));
        div()
            .relative()
            .h(px(BREADCRUMBS_HEIGHT))
            .flex_none()
            // The breadcrumbs sit in the title (editortabscontrol.css) and each item is pointer (breadcrumbsWidget.css).
            .cursor_pointer()
            .flex()
            .bg(self.color("breadcrumb.background"))
            .text_color(foreground)
            .children(items)
            .when_some(title_border, |crumbs, color| crumbs.child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(color)))
    }

    fn text_area(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let metrics = EditorMetrics::measure(window, editor.document.lines.len(), self.editor_zoom.multiplier());
        let area_left = self.text_area_left.clone();
        let area_bounds = self.text_area_bounds.clone();
        let focus = self.editor_focus.clone();
        let workbench = cx.entity();
        let laid_out = cx.entity();
        let view_line_count = editor.view_line_count();
        let sticky_scroll_lines = self.sticky_scroll_lines();
        let top_whitespace = self.editors.viewport.top_whitespace;
        let stop_rendering_line_after = self.stop_rendering_line_after;
        let minimap_enabled = self.setting("editor.minimap.enabled") != json!(false);
        let viewport = self.editors.viewport;
        let lines = editor.visible_lines(viewport);
        let lines_top = viewport.line_top(lines.start) - editor.scroll.top;
        let rows = self.editor_lines(editor, lines, metrics, window, cx);
        let minimap_width = self.minimap.info.as_ref().map_or(0., |(info, _)| info.width as f32);
        let now = Instant::now();
        if self.scrollbars.in_transition(now) || self.fold_controls.in_transition(now) || self.minimap.in_transition(now) {
            window.request_animation_frame();
        }
        let editor_origin = self.text_area_bounds.get().origin;
        let middle_scroll_dot = self.middle_scroll_dot(point(px(editor_origin.x), px(editor_origin.y)));
        let middle_scrolling = middle_scroll_dot.is_some();
        let scrolled = cx.entity();
        div()
            .id("text-area")
            .flex_1()
            .relative()
            .overflow_hidden()
            .font_family(EDITOR_FONT)
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            .track_focus(&self.editor_focus)
            .on_key_down(cx.listener(Self::editor_key_down))
            .on_scroll_wheel(cx.listener(Self::editor_wheel))
            .on_modifiers_changed(cx.listener(Self::sticky_modifiers_changed))
            .on_modifiers_changed(cx.listener(|workbench, event: &gpui::ModifiersChangedEvent, _, cx| workbench.text_drag_modifiers_changed(event.modifiers, cx)))
            .on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                workbench.scrollbars.mouse_over = *hovered;
                match hovered {
                    true => workbench.reveal_scrollbars(cx),
                    false => workbench.hide_scrollbars(cx),
                }
            }))
            .capture_any_mouse_down(cx.listener(|workbench, event: &MouseDownEvent, _, cx| {
                if workbench.middle_scroll_mouse_down(event.button == MouseButton::Middle, event.position, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_any_mouse_up(cx.listener(|workbench, _, _, cx| workbench.middle_scroll_mouse_up(cx)))
            .capture_key_down(cx.listener(|workbench, _, _, cx| {
                workbench.end_middle_scroll(cx);
                workbench.cancel_text_drag(cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|workbench, _, _, cx| {
                workbench.selecting_text = false;
                workbench.cancel_text_drag(cx);
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|workbench, _, _, cx| {
                workbench.selecting_text = false;
                workbench.cancel_text_drag(cx);
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        area_left.set(f32::from(bounds.left()));
                        area_bounds.set(bounds.map(f32::from));
                        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        let pixel_ratio = window.scale_factor();
                        let minimap = minimap_enabled.then(|| minimap_info(width, height, metrics.content_left(), metrics.char_width, metrics.line_height, view_line_count, pixel_ratio));
                        // editorOptions.ts: contentWidth = remainingWidth - minimapWidth.
                        let minimap_width = minimap.as_ref().map_or(0., |minimap| minimap.width as f32);
                        let viewport = Viewport { width: width - metrics.content_left() - minimap_width, height, char_width: metrics.char_width, line_height: metrics.line_height, sticky_scroll_lines, top_whitespace, stop_rendering_line_after };
                        laid_out.update(cx, |workbench, cx| workbench.editor_laid_out(viewport, minimap.map(|minimap| (minimap, pixel_ratio as f64)), cx));
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, workbench), cx);
                        if middle_scrolling && scrolled.update(cx, |workbench, cx| workbench.middle_scroll_frame(cx)) {
                            // observeWindowMousePos: the pointer anywhere in the window steers the session.
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    scrolled.update(cx, |workbench, _| workbench.middle_scroll_mouse_moved(event.position));
                                }
                            });
                            window.request_animation_frame();
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            // .view-lines is monaco-mouse-cursor-text; the line numbers, glyph margin and the overview ruler over the
            // vertical scrollbar keep the default cursor.
            .child(div().absolute().top_0().bottom_0().left(px(metrics.content_left())).right(px(OVERVIEW_RULER_WIDTH)).cursor(self.text_cursor_style()).on_mouse_down(
                MouseButton::Right,
                // Below the last line (MouseTargetType.CONTENT_EMPTY): the position is the end of the last line.
                cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                    let last = workbench.editors.active().expect("the text area shows an editor").document.lines.len() - 1;
                    workbench.show_editor_context_menu(last, f32::MAX, event.position, window, cx);
                }),
            ))
            // editorScrollbar.ts clips the lines at contentWidth: the column right of the minimap stays empty.
            .child(div().absolute().left_0().right(px(minimap_width)).top_0().bottom_0().overflow_hidden().child(div().absolute().left_0().right_0().top(px(lines_top)).children(rows)))
            .child(
                div().id("margin").absolute().top_0().bottom_0().left_0().w(px(metrics.content_left())).on_hover(cx.listener(|workbench, hovered: &bool, _, cx| {
                    workbench.fold_controls.set_shown(*hovered, Instant::now());
                    cx.notify();
                })),
            )
            .child(self.overview_ruler(editor).absolute().top_0().right_0().h_full())
            .child(self.scrollbars_canvas(editor, metrics, cx).absolute().inset_0())
            .when(editor.scroll.top > 0., |area| area.child(div().absolute().top_0().left_0().right(px(VERTICAL_SCROLLBAR_WIDTH)).h(px(SCROLL_SHADOW_HEIGHT)).cursor_default().child(self.scroll_shadow().size_full())))
            .children(self.minimap(editor, cx))
            .children(middle_scroll_dot)
            .children(self.sticky_scroll_widget(editor, metrics, window, cx))
            .children(self.find_widget(self.editors.viewport.width + metrics.content_left() + minimap_width, metrics.content_left(), minimap_width, window, cx))
            .children(self.editor_message_widget(editor, metrics.content_left(), metrics.char_width, window, cx))
    }

    /// wordHighlighter.ts: `onDidBlurEditorWidget` stops the highlights once no code editor has the focus,
    /// `onDidFocusEditorText` runs the query from the selection again. The find widget, sticky lines and the
    /// unicode banner are inside the widget.
    pub fn watch_editor_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.editor_focus.clone();
        let blurred_at = Rc::new(Cell::new(None));
        let deactivated = Rc::new(Cell::new(false));
        let blur = cx.on_focus_out(&focus, window, {
            let (blurred_at, deactivated) = (blurred_at.clone(), deactivated.clone());
            move |workbench, _, window, cx| {
                // GPUI reports a window deactivation as focus out; the DOM keeps the focus there, so the
                // reactivation is no focus event either.
                if workbench.editor_focus.contains_focused(window, cx) {
                    return deactivated.set(true);
                }
                workbench.editor_message_text_blurred(cx);
                if !(workbench.unicode_banner_focus.is_focused(window) && workbench.unicode_banner_shown()) {
                    workbench.editor_widget_blurred(&blurred_at, cx);
                }
            }
        });
        // The banner holding the focus goes when its action disables the highlights: the focus leaves the editor.
        let banner_blur = cx.on_blur(&self.unicode_banner_focus, window, {
            let blurred_at = blurred_at.clone();
            move |workbench, window, cx| {
                if window.is_window_active() && !workbench.editor_focus.contains_focused(window, cx) {
                    workbench.editor_widget_blurred(&blurred_at, cx);
                }
            }
        });
        let text_blur = cx.on_blur(&focus, window, |workbench, window, cx| {
            if window.is_window_active() {
                workbench.multicursor_text_focus_changed(false, window, cx);
            }
        });
        let text_focus = cx.on_focus(&focus, window, move |workbench, window, cx| {
            let Some(editor) = workbench.editors.active_mut() else { return };
            // A picked command runs before GPUI reports the restored focus; upstream's delayed query, started on
            // focus, is cancelled by that command's edit (`onDidChangeModelContent` stops it).
            let returned = blurred_at.take().is_none_or(|(model, version)| model != editor.model_id || version == editor.version_id);
            let reactivated = deactivated.take();
            if returned && !reactivated {
                editor.refresh_highlights();
            }
            if !reactivated {
                workbench.multicursor_text_focus_changed(true, window, cx);
            }
            cx.notify();
        });
        self.editor_focus_listeners = vec![blur, banner_blur, text_blur, text_focus];
    }

    fn editor_widget_blurred(&mut self, blurred_at: &Cell<Option<(u32, u32)>>, cx: &mut Context<Self>) {
        self.text_drag = None;
        if let Some(editor) = self.editors.active_mut() {
            editor.highlights.clear();
            blurred_at.set(Some((editor.model_id, editor.version_id)));
        }
        cx.notify();
    }

    /// The text area got its size: the group's viewport follows it, which can clamp the scroll position and
    /// change the visible lines; the lines drawn widen the scrollable content (`ViewLines._ensureMaxLineWidth`).
    fn editor_laid_out(&mut self, viewport: Viewport, minimap: Option<(MinimapLayoutInfo, f64)>, cx: &mut Context<Self>) {
        let resized = self.editors.viewport != viewport;
        let font_changed = (self.editors.viewport.char_width, self.editors.viewport.line_height) != (viewport.char_width, viewport.line_height);
        let minimap_changed = self.minimap.info != minimap;
        self.editors.viewport = viewport;
        self.minimap.info = minimap;
        let editor = self.editors.active_mut().expect("laid out only with an editor open");
        let scroll_width = editor.scroll_width(viewport);
        let visible_lines = editor.visible_lines(viewport);
        // viewLines.ts `_updateLineWidths`: with every line drawn, their widest is the content width.
        if font_changed || visible_lines == (0..editor.view_line_count()) {
            editor.forget_line_widths();
        }
        for view_line in visible_lines {
            let text = &editor.document.lines[editor.model_line(view_line)];
            let tab_size = editor.document.indentation.tab_size;
            let width = match line_overflow(text, self.stop_rendering_line_after) {
                None => visible_column(text, text.chars().count(), tab_size) as f32 * viewport.char_width,
                Some((limit, _, label)) => visible_column(text, limit, tab_size) as f32 * viewport.char_width + show_more_width(&label, viewport.char_width),
            };
            editor.saw_line_width(width);
        }
        let moved = editor.set_scroll(editor.scroll.top, editor.scroll.left, viewport);
        let widened = editor.scroll_width(viewport) != scroll_width;
        if resized || moved {
            self.host.send_visible_ranges(&self.editors);
        }
        if resized || moved || widened || minimap_changed {
            cx.notify();
        }
    }

    /// The wheel scrolls the editor in `StandardWheelEvent` units: 40 browser pixels or one line per notch. With
    /// `editor.mouseWheelZoom`, Cmd or Ctrl without Shift and Alt zooms instead (mouseHandler.ts).
    fn editor_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x) / 40., f32::from(delta.y) / 40.),
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        if self.setting("editor.mouseWheelZoom").as_bool().unwrap_or(false) {
            let modifiers = event.modifiers;
            let has_zoom_modifiers = (modifiers.platform || modifiers.control) && !modifiers.shift && !modifiers.alt;
            // ScrollDelta::Lines is AppKit's imprecise delta of a mouse wheel, `isPhysicalMouseWheel` upstream.
            let physical_wheel = matches!(event.delta, ScrollDelta::Lines(_));
            let level = self.editor_zoom.level();
            if self.editor_zoom.wheel(y, physical_wheel, has_zoom_modifiers, Instant::now()) {
                cx.stop_propagation();
                if self.editor_zoom.level() != level {
                    cx.notify();
                }
                return;
            }
        }
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("the text area scrolls only with an editor open");
        if editor.scroll_by_wheel(x, y, event.modifiers.alt, viewport) {
            cx.stop_propagation();
            self.editor_scrolled(cx);
        }
    }

    /// scrollableElement.ts `_onDidScroll`: the scrollbars show, and the host hears of new visible lines.
    pub(super) fn editor_scrolled(&mut self, cx: &mut Context<Self>) {
        self.host.send_visible_ranges(&self.editors);
        self.reveal_scrollbars(cx);
        self.minimap_scrolled(cx);
        cx.notify();
    }

    /// `_reveal`: the scrollbars fade in, and fade out 500ms later unless the pointer is over the editor.
    fn reveal_scrollbars(&mut self, cx: &mut Context<Self>) {
        self.scrollbars.set_shown(true, Instant::now());
        self.scrollbars.hide_timer = None;
        if !self.scrollbars.mouse_over && self.scrollbars.drag.is_none() {
            self.scrollbars.hide_timer = Some(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(SCROLLBAR_HIDE_TIMEOUT).await;
                workbench.update(cx, |workbench, cx| workbench.hide_scrollbars(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// `_hide`: kept while the pointer is over the editor or drags a slider.
    fn hide_scrollbars(&mut self, cx: &mut Context<Self>) {
        if !self.scrollbars.mouse_over && self.scrollbars.drag.is_none() {
            self.scrollbars.set_shown(false, Instant::now());
            cx.notify();
        }
    }

    /// The sliders over the overview ruler and along the bottom (scrollbars.css, abstractScrollbar.ts): a press
    /// on a track centers the slider on the pointer, then the slider follows the pointer until release.
    fn scrollbars_canvas(&self, editor: &OpenEditor, metrics: EditorMetrics, cx: &mut Context<Self>) -> gpui::Canvas<Vec<(Hitbox, Bounds<Pixels>, Axis)>> {
        let viewport = self.editors.viewport;
        let bars = [(Axis::Vertical, editor.vertical_scrollbar(viewport)), (Axis::Horizontal, editor.horizontal_scrollbar(viewport))];
        let opacity = self.scrollbars.opacity(Instant::now());
        let shown = self.scrollbars.shown;
        let drag = self.scrollbars.drag;
        let faded = |key: &str| {
            let color = self.color(key);
            Rgba { a: color.a * opacity, ..color }
        };
        let colors = [faded("scrollbarSlider.background"), faded("scrollbarSlider.hoverBackground"), faded("scrollbarSlider.activeBackground")];
        let workbench = cx.entity();
        canvas(
            move |bounds, window, _| {
                let mut tracks = Vec::new();
                for (axis, bar) in bars {
                    let track = match axis {
                        Axis::Vertical => Bounds::new(point(bounds.right() - px(VERTICAL_SCROLLBAR_WIDTH), bounds.top()), size(px(VERTICAL_SCROLLBAR_WIDTH), px(bar.track))),
                        Axis::Horizontal => Bounds::new(point(bounds.left() + px(metrics.content_left()), bounds.bottom() - px(HORIZONTAL_SCROLLBAR_HEIGHT)), size(px(bar.track), px(HORIZONTAL_SCROLLBAR_HEIGHT))),
                    };
                    // An invisible scrollbar takes no pointer events (.invisible { pointer-events: none }).
                    if bar.needed && shown {
                        tracks.push((window.insert_hitbox(track, HitboxBehavior::BlockMouse), track, axis));
                    }
                }
                tracks
            },
            move |_, tracks, window, _| {
                let mut sliders = Vec::new();
                for (axis, bar) in bars {
                    let Some((_, track, _)) = tracks.iter().find(|(_, _, track_axis)| *track_axis == axis) else { continue };
                    let slider = slider_bounds(*track, axis, bar);
                    let color = match drag {
                        Some(drag) if drag.axis == axis => colors[2],
                        _ if slider.contains(&window.mouse_position()) => colors[1],
                        _ => colors[0],
                    };
                    window.paint_quad(fill(slider, color));
                    sliders.push(slider);
                }
                let hovered_slider = move |window: &Window| sliders.iter().any(|slider| slider.contains(&window.mouse_position()));
                let painted_hover = hovered_slider(window);
                let workbench_moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if let Some(drag) = drag {
                        let along = if drag.axis == Axis::Vertical { event.position.y } else { event.position.x };
                        workbench_moved.update(cx, |workbench, cx| workbench.drag_slider(drag, f32::from(along), cx));
                    } else if hovered_slider(window) != painted_hover {
                        window.refresh();
                    }
                });
                let workbench_released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && drag.is_some() {
                        workbench_released.update(cx, |workbench, cx| {
                            workbench.scrollbars.drag = None;
                            workbench.hide_scrollbars(cx);
                        });
                    }
                });
                let workbench = workbench.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let Some((_, track, axis)) = tracks.iter().find(|(hitbox, _, _)| hitbox.is_hovered(window)) else { return };
                    let (track, axis) = (*track, *axis);
                    let along = |position: Point<Pixels>| f32::from(if axis == Axis::Vertical { position.y - track.top() } else { position.x - track.left() });
                    let offset = along(event.position);
                    cx.stop_propagation();
                    workbench.update(cx, |workbench, cx| workbench.press_scrollbar(axis, offset, f32::from(if axis == Axis::Vertical { event.position.y } else { event.position.x }), cx));
                });
            },
        )
    }

    /// A press on a track: outside the slider it jumps to center the slider there, then a drag starts.
    fn press_scrollbar(&mut self, axis: Axis, offset: f32, pointer: f32, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("scrollbars show only with an editor open");
        let bar = scrollbar(editor, axis, viewport);
        let on_slider = (bar.slider_position..bar.slider_position + bar.slider_size).contains(&offset);
        if !on_slider {
            set_axis_scroll(editor, axis, bar.position_at(offset), viewport);
        }
        let state = scrollbar(editor, axis, viewport);
        self.scrollbars.drag = Some(SliderDrag { axis, pointer, state });
        self.editor_scrolled(cx);
    }

    fn drag_slider(&mut self, drag: SliderDrag, pointer: f32, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("scrollbars show only with an editor open");
        if set_axis_scroll(editor, drag.axis, drag.state.position_dragged(pointer - drag.pointer), viewport) {
            self.editor_scrolled(cx);
        }
    }

    /// scrollDecoration.css: `box-shadow: var(--vscode-scrollbar-shadow) 0 6px 6px -6px inset` over a 6px strip,
    /// which leaves the blurred top edge only: alpha falls off as a Gaussian with sigma 3px, half strength at the edge.
    fn scroll_shadow(&self) -> gpui::Canvas<()> {
        let shadow = self.theme.color("scrollbar.shadow");
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let Some(shadow) = shadow else { return };
                let scale = window.scale_factor();
                let rows = (SCROLL_SHADOW_HEIGHT * scale) as usize;
                for row in 0..rows {
                    let y = (row as f32 + 0.5) / scale;
                    let alpha = shadow.a * 0.5 * erfc(y / (3. * std::f32::consts::SQRT_2));
                    let origin = bounds.origin + point(px(0.), px(row as f32 / scale));
                    window.paint_quad(fill(Bounds::new(origin, size(bounds.size.width, px(1. / scale))), Rgba { a: alpha, ..shadow }));
                }
            },
        )
    }

    fn editor_lines(&self, editor: &OpenEditor, range: Range<usize>, metrics: EditorMetrics, window: &mut Window, cx: &mut Context<Self>) -> Vec<Div> {
        let focused = self.editor_focus.is_focused(window);
        let scroll_left = editor.scroll.left;
        let selections = &editor.selections;
        let tab_size = editor.document.indentation.tab_size;
        let primary_line = editor.selection().head.line;
        // currentLineHighlight.ts: every cursor's line, drawn only while all selections are empty.
        let all_empty = selections.iter().all(|selection| selection.is_empty());
        let bracket_matches = self.bracket_matches(editor);
        // Past the cap there are no marks, the banner shows instead.
        let unicode = self.unicode_highlights(editor).filter(|(_, highlights)| !highlights.has_more);
        let bracket_box = (self.theme.color("editorBracketMatch.background"), self.theme.color("editorBracketMatch.border"));
        let colors = LineColors {
            number: self.color("editorLineNumber.foreground"),
            active_number: self.color("editorLineNumber.activeForeground"),
            text: self.color("editor.foreground"),
            line_border: self.color("editor.lineHighlightBorder"),
            word: self.theme.color("editor.wordHighlightTextBackground"),
            word_border: self.theme.color("editor.wordHighlightTextBorder"),
            selection_highlight: self.theme.color("editor.selectionHighlightBackground"),
            selection_highlight_border: self.theme.color("editor.selectionHighlightBorder"),
            guide: self.color("editorIndentGuide.background1"),
            cursor: self.color("editorCursor.foreground"),
            primary_cursor: self.color("editorMultiCursor.primary.foreground"),
            secondary_cursor: self.color("editorMultiCursor.secondary.foreground"),
            selection: self.color(if focused { "editor.selectionBackground" } else { "editor.inactiveSelectionBackground" }),
        };
        // currentLineHighlight.css: high contrast draws the exact border 1px wide.
        let line_border_width = if self.theme.kind.is_high_contrast() { 1. } else { 2. };
        // editorOptions.ts: overtype input mode draws `overtypeCursorStyle`, else `cursorStyle`; only block and line are drawn.
        let block_cursor = self.setting(if self.overtype { "editor.overtypeCursorStyle" } else { "editor.cursorStyle" }) == json!("block");
        let cursor_background = self.theme.color("editorCursor.background");
        let char_width = metrics.char_width;
        let fold_controls = self.fold_controls.opacity(Instant::now());
        let fold_control_color = self.color("editorGutter.foldingControlForeground");
        let fold_background = self.color("editor.foldBackground");
        let fold_placeholder = self.color("editor.foldPlaceholderForeground");
        let rounded_selection = self.setting("editor.roundedSelection") != json!(false);
        let render_whitespace = self.setting("editor.renderWhitespace");
        let render_whitespace = render_whitespace.as_str().unwrap_or_default();
        let whitespace_color = self.color("editorWhitespace.foreground");
        let line_height = metrics.line_height;
        // selections.css: high contrast themes square the corners.
        let selection_radius = if self.theme.kind.is_high_contrast() { 0. } else { 3. };
        let editor_background = self.color("editor.background");
        let view_line_count = editor.view_line_count();
        // viewLines.ts linesVisibleRangesForRange: a selected line break is one typicalHalfwidthCharacterWidth wide.
        let selection_range = |selection: Selection, view_line: usize| {
            let line = editor.model_line(view_line);
            let text = &editor.document.lines[line];
            let (start, end, line_break) = selected_columns(selection, line, text)?;
            let left = visible_column(text, start, tab_size) as f32 * char_width;
            Some((left, visible_column(text, end, tab_size) as f32 * char_width + if line_break { char_width } else { 0. }))
        };
        let mut fold_starts = HashMap::new();
        for (range, decoration) in editor.folding.regions.ranges().iter().zip(editor.folding.decorations()) {
            if decoration != FoldDecoration::Hidden {
                fold_starts.insert(range.start - 1, decoration == FoldDecoration::Collapsed);
            }
        }
        range
            .map(|view_line| {
                let index = editor.model_line(view_line);
                let text = &editor.document.lines[index];
                let collapsed = fold_starts.get(&index).copied();
                // lineNumbers.ts marks only the primary cursor's line active.
                let number = div()
                    .w(px(metrics.line_numbers_width))
                    .flex_none()
                    .text_right()
                    .text_color(if index == primary_line { colors.active_number } else { colors.number })
                    .child((index + 1).to_string());
                let mut current_line = div().absolute().inset_0();
                if all_empty && selections.iter().any(|selection| selection.head.line == index) {
                    current_line = current_line.border(px(line_border_width)).border_color(colors.line_border);
                }
                // The text scrolls left under the fixed margin and current line border.
                let mut content = div().absolute().top_0().bottom_0().right_0().left(px(-scroll_left));
                // selections.ts: inner corner pieces of every selection go before the rest (issue #77777).
                let mut inner_corners = Vec::new();
                let mut pieces = Vec::new();
                for selection in selections {
                    let Some((left, right)) = selection_range(*selection, view_line) else { continue };
                    let above = view_line.checked_sub(1).and_then(|above| selection_range(*selection, above));
                    let below = if view_line + 1 < view_line_count { selection_range(*selection, view_line + 1) } else { None };
                    // `hasMultipleSelections` insets the first line by 1px at the top, the last (not also first) at the bottom.
                    let multiple = selections.len() > 1;
                    let top = if multiple && above.is_none() { 1. } else { 0. };
                    let bottom = if multiple && above.is_some() && below.is_none() { 1. } else { 0. };
                    let piece = |left: f32, width: f32| div().absolute().top(px(top)).bottom(px(bottom)).left(px(left)).w(px(width));
                    let mut rest = piece(left, right - left).bg(colors.selection);
                    if rounded_selection {
                        let corners = selection_corners(above, (left, right), below, char_width / 4.);
                        let radius = px(selection_radius);
                        if corners.start_top == Corner::Intern || corners.start_bottom == Corner::Intern {
                            let mut background = piece(left - ROUNDED_PIECE_WIDTH, ROUNDED_PIECE_WIDTH).bg(editor_background);
                            if corners.start_top == Corner::Intern {
                                background = background.rounded_tr(radius);
                            }
                            if corners.start_bottom == Corner::Intern {
                                background = background.rounded_br(radius);
                            }
                            inner_corners.push(piece(left - ROUNDED_PIECE_WIDTH, ROUNDED_PIECE_WIDTH).bg(colors.selection));
                            inner_corners.push(background);
                        }
                        if corners.end_top == Corner::Intern || corners.end_bottom == Corner::Intern {
                            let mut background = piece(right, ROUNDED_PIECE_WIDTH).bg(editor_background);
                            if corners.end_top == Corner::Intern {
                                background = background.rounded_tl(radius);
                            }
                            if corners.end_bottom == Corner::Intern {
                                background = background.rounded_bl(radius);
                            }
                            inner_corners.push(piece(right, ROUNDED_PIECE_WIDTH).bg(colors.selection));
                            inner_corners.push(background);
                        }
                        if corners.start_top == Corner::Extern {
                            rest = rest.rounded_tl(radius);
                        }
                        if corners.start_bottom == Corner::Extern {
                            rest = rest.rounded_bl(radius);
                        }
                        if corners.end_top == Corner::Extern {
                            rest = rest.rounded_tr(radius);
                        }
                        if corners.end_bottom == Corner::Extern {
                            rest = rest.rounded_br(radius);
                        }
                    }
                    pieces.push(rest);
                }
                content = content.children(inner_corners).children(pieces);
                for level in 0..editor.guides[index] {
                    let x = (level * editor.document.indentation.indent_size()) as f32 * char_width;
                    content = content.child(div().absolute().top_0().left(px(x)).w(px(1.)).h_full().bg(colors.guide));
                }
                // foldingDecorations.ts COLLAPSED_HIGHLIGHTED_VISUAL_DECORATION: .folded-background over the whole line.
                if collapsed == Some(true) {
                    content = content.child(div().absolute().inset_0().left(px(scroll_left)).bg(fold_background));
                }
                let (find_whole_lines, find_matches, find_colors) = self.find_line_decorations(editor, index, char_width, scroll_left);
                content = content.children(find_whole_lines);
                // anchorSelect.css: `.selection-anchor` is a 2px #007ACC bar in every theme, under the word highlights.
                if let Some(anchor) = editor.selection_anchor
                    && anchor.line == index
                {
                    let left = visible_column(text, anchor.column, tab_size) as f32 * char_width;
                    content = content.child(div().absolute().top_0().h_full().left(px(left)).w(px(2.)).bg(rgb(0x007ACC)));
                }
                // highlightDecorations.css: `.focused .selectionHighlight`, drawn under the word highlights.
                let selection_highlights = self.find.selection_highlights.as_ref().filter(|_| focused).map_or(&[][..], |(_, ranges)| ranges);
                let first = selection_highlights.partition_point(|range| range.end.line < index);
                let on_line = selection_highlights[first..].iter().take_while(|range| range.start.line <= index);
                for (start, end, _) in on_line.filter_map(|range| selected_columns(Selection { anchor: range.start, head: range.end }, index, text)) {
                    let start = visible_column(text, start, tab_size) as f32 * char_width;
                    let end = visible_column(text, end, tab_size) as f32 * char_width;
                    let mut highlight = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = colors.selection_highlight {
                        highlight = highlight.bg(background);
                    }
                    // shortcut: GPUI has no dotted border, so the high contrast selection highlight is dashed.
                    if let Some(border) = colors.selection_highlight_border {
                        highlight = highlight.border_1().border_dashed().border_color(border);
                    }
                    content = content.child(highlight);
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
                // bracketMatching.css .bracket-match: background and a 1px border inside the bracket's box.
                for (_, bytes) in bracket_matches.iter().flat_map(|(pair, _)| pair).filter(|(line, _)| *line == index) {
                    let start = visible_column(text, text[..bytes.start].chars().count(), tab_size) as f32 * char_width;
                    let end = visible_column(text, text[..bytes.end].chars().count(), tab_size) as f32 * char_width;
                    let mut bracket = div().absolute().top_0().h_full().left(px(start)).w(px(end - start));
                    if let Some(background) = bracket_box.0 {
                        bracket = bracket.bg(background);
                    }
                    if let Some(border) = bracket_box.1 {
                        bracket = bracket.border_1().border_color(border);
                    }
                    content = content.child(bracket);
                }
                content = content.children(self.unicode_highlight_boxes(editor, index, unicode.as_ref(), char_width));
                content = content.children(find_matches);
                // inPlaceReplace.css: .valueSetReplacement only in the light theme (`.monaco-editor.vs`), a 2px outline.
                if let Some((_, line, columns)) = &editor.value_set_replacement
                    && *line == index
                    && self.theme.kind == ThemeKind::Light
                    && let Some(border) = bracket_box.1
                {
                    let start = visible_column(text, columns.start, tab_size) as f32 * char_width;
                    let end = visible_column(text, columns.end, tab_size) as f32 * char_width;
                    content = content.child(div().absolute().top(px(-2.)).bottom(px(-2.)).left(px(start - 2.)).w(px(end - start + 4.)).border_2().border_color(border));
                }
                let selected: Vec<_> = selections
                    .iter()
                    .filter_map(|selection| selected_columns(*selection, index, text))
                    .filter(|(start, end, _)| start < end)
                    .map(|(start, end, _)| start..end)
                    .collect();
                let rendered_len = line_overflow(text, self.stop_rendering_line_after).map_or(text.chars().count(), |(limit, _, _)| limit);
                let marks = whitespace_marks(text, render_whitespace, &selected, rendered_len);
                if !marks.is_empty() {
                    let marks: Vec<_> = marks.into_iter().map(|mark| (visible_column(text, mark, tab_size) as f32 * char_width, text.chars().nth(mark) == Some('\t'))).collect();
                    content = content.child(whitespace_canvas(marks, char_width, line_height, whitespace_color));
                }
                let highlights = editor.document.tokens.highlights(&self.theme, &self.languages, &editor.document.lines, index, tab_size);
                let runs = gpui::combine_highlights(highlights, find_colors);
                let mut show_more = None;
                match line_overflow(text, self.stop_rendering_line_after) {
                    None => content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(expand_tabs(text, tab_size)).with_highlights(runs))),
                    Some((limit, byte, label)) => {
                        // viewLineRenderer.ts: the first `stopRenderingLineAfter` chars, then the .mtkoverflow "Show more" span.
                        let rendered = expand_tabs(&text[..byte], tab_size);
                        let len = rendered.len();
                        let runs: Vec<_> = runs.filter(|(range, _)| range.start < len).map(|(range, style)| (range.start..range.end.min(len), style)).collect();
                        content = content.child(div().absolute().top_0().left_0().whitespace_nowrap().child(StyledText::new(rendered).with_highlights(runs)));
                        let foreground = self.theme.color("button.foreground").unwrap_or(colors.text);
                        let span = div()
                            .absolute()
                            .left(px(visible_column(text, limit, tab_size) as f32 * char_width - scroll_left))
                            .w(px(show_more_width(&label, char_width)))
                            .h_full()
                            .rounded(px(2.))
                            .border_1()
                            // An unset `--vscode-contrastBorder` leaves the border `currentColor`.
                            .border_color(self.theme.color("contrastBorder").unwrap_or(foreground))
                            .bg(self.theme.color("button.background").unwrap_or(editor_background))
                            .text_color(foreground)
                            .when_some(self.theme.color("button.hoverBackground"), |span, hover| span.hover(move |span| span.bg(hover)))
                            .cursor_pointer()
                            .child(div().absolute().top(px(2.)).left(px(4.)).line_height(px(metrics.line_height)).whitespace_nowrap().child(label));
                        // The inline span's box reaches 3px above the line and 2px below it (reference pixels), past the
                        // line's own clip, so it gets a clip of its own over the text area.
                        show_more = Some(div().absolute().left(px(metrics.content_left())).right_0().top(px(-3.)).h(px(24.)).overflow_hidden().child(span));
                    }
                }
                // folding.css .inline-folded:after: an ellipsis 0.2em after the line's last char.
                if collapsed == Some(true) {
                    let end = visible_column(text, text.chars().count(), tab_size) as f32 * char_width + 0.2 * metrics.font_size;
                    content = content.child(div().absolute().top_0().left(px(end)).text_color(fold_placeholder).cursor_pointer().child("\u{22EF}"));
                }
                for (cursor_index, selection) in selections.iter().enumerate().filter(|(_, selection)| focused && selection.head.line == index) {
                    // shortcut: the cursor does not blink (editor.cursorBlinking "blink" hides it every 500ms).
                    let color = match (selections.len(), cursor_index) {
                        (1, _) => colors.cursor,
                        (_, 0) => colors.primary_cursor,
                        _ => colors.secondary_cursor,
                    };
                    // A cursor past the cut is not drawn.
                    if self.stop_rendering_line_after.is_some_and(|limit| selection.head.column > limit) {
                        continue;
                    }
                    let x = visible_column(text, selection.head.column, tab_size) as f32 * char_width;
                    if block_cursor {
                        // viewCursor.ts block: one char wide (a tab too), the char under it in `editorCursor.background`,
                        // else the caret color's opposite (viewCursors.ts).
                        let under = text.chars().nth(selection.head.column).filter(|ch| *ch != '\t').map(String::from).unwrap_or_default();
                        let text_color = cursor_background.unwrap_or(gpui::Rgba { r: 1. - color.r, g: 1. - color.g, b: 1. - color.b, a: color.a });
                        content = content.child(div().absolute().top_0().left(px(x)).w(px(char_width)).h_full().bg(color).text_color(text_color).child(under));
                        continue;
                    }
                    // viewCursor.ts: a 2px cursor sits 1px left, between the chars.
                    let left = if x >= 1. { x - 1. } else { x };
                    content = content.child(div().absolute().top_0().left(px(left)).w(px(2.)).h_full().bg(color));
                }
                if let Some(target) = self.text_drag.as_ref().and_then(|drag| drag.target).filter(|target| target.line == index) {
                    content = content.child(self.drop_target_marker(visible_column(text, target.column, tab_size) as f32 * char_width));
                }
                let visible_at = move |workbench: &Workbench, x: gpui::Pixels| (f32::from(x) - workbench.text_area_left.get() - metrics.content_left() + scroll_left) / char_width;
                let content = div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .text_color(colors.text)
                    .child(current_line)
                    .child(content)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        window.focus(&workbench.editor_focus, cx);
                        let modifiers = event.modifiers;
                        let visible = visible_at(workbench, event.position.x);
                        if workbench.start_text_drag(index, visible, modifiers, event.click_count) {
                            return;
                        }
                        workbench.selecting_text = true;
                        workbench.fold_mouse_down = workbench.at_line_end(index, visible).then_some((index, false));
                        workbench.click_text(index, visible, modifiers, modifiers.shift, event.click_count, cx);
                        workbench.show_long_line(index, visible, char_width, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        let visible = visible_at(workbench, event.position.x);
                        workbench.show_editor_context_menu(index, visible, event.position, window, cx);
                        workbench.show_long_line(index, visible, char_width, cx);
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        window.focus(&workbench.editor_focus, cx);
                        workbench.selecting_text = true;
                        let visible = visible_at(workbench, event.position.x);
                        workbench.fold_mouse_down = workbench.at_line_end(index, visible).then_some((index, false));
                        workbench.column_select_text(index, visible, event.modifiers.shift, cx);
                        workbench.show_long_line(index, visible, char_width, cx);
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| {
                        if workbench.text_drag.is_some() {
                            return workbench.drop_text(index, visible_at(workbench, event.position.x), event.modifiers, cx);
                        }
                        if workbench.at_line_end(index, visible_at(workbench, event.position.x)) {
                            workbench.release_fold_click(index, false, event.modifiers, false, cx);
                        }
                    }))
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| {
                        if workbench.at_line_end(index, visible_at(workbench, event.position.x)) {
                            workbench.release_fold_click(index, false, event.modifiers, true, cx);
                        }
                    }))
                    .on_mouse_move(cx.listener(move |workbench, event: &MouseMoveEvent, _, cx| {
                        workbench.editor_message_mouse_moved(index, cx);
                        match event.pressed_button {
                            Some(MouseButton::Left) if workbench.text_drag.is_some() => workbench.drag_text(index, visible_at(workbench, event.position.x), event.modifiers.alt, cx),
                            Some(MouseButton::Left) if workbench.selecting_text => workbench.click_text(index, visible_at(workbench, event.position.x), event.modifiers, true, 1, cx),
                            Some(MouseButton::Middle) if workbench.selecting_text => workbench.column_select_text(index, visible_at(workbench, event.position.x), true, cx),
                            _ => {}
                        }
                    }));
                div()
                    .w_full()
                    .h(px(metrics.line_height))
                    .flex()
                    .child(div().w(px(metrics.glyph_margin_width())).flex_none())
                    .child(number)
                    .child(self.fold_control(index, collapsed, metrics, fold_control_color, fold_controls, cx))
                    .child(content)
                    .children(show_more)
            })
            .collect()
    }

    /// The line decorations lane with the folding control of a region starting on `index`, collapsed or not
    /// (`folding-collapsed` stays visible, `folding-expanded` shows while the pointer is over the margin). A press
    /// 4px or more into the lane and a release on the same line toggle the region (folding.ts `onEditorMouseDown`).
    fn fold_control(&self, index: usize, collapsed: Option<bool>, metrics: EditorMetrics, color: Rgba, hover_opacity: f32, cx: &mut Context<Self>) -> Div {
        let lane_x = move |workbench: &Workbench, x: Pixels| f32::from(x) - workbench.text_area_left.get() - metrics.glyph_margin_width() - metrics.line_numbers_width;
        let press = move |workbench: &mut Workbench, event: &MouseDownEvent| {
            workbench.fold_mouse_down = (lane_x(workbench, event.position.x) >= 4.).then_some((index, true));
        };
        let mut lane = div()
            .w(px(DECORATIONS_WIDTH))
            .flex_none()
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, _| press(workbench, event)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |workbench, event: &MouseDownEvent, _, _| press(workbench, event)))
            .on_mouse_up(MouseButton::Left, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| workbench.release_fold_click(index, true, event.modifiers, false, cx)))
            .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, event: &MouseUpEvent, _, cx| workbench.release_fold_click(index, true, event.modifiers, true, cx)));
        let Some(collapsed) = collapsed else { return lane };
        let (name, opacity) = if collapsed { ("chevron-right", 1.) } else { ("chevron-down", hover_opacity) };
        // folding.css: the icon box is the lane's width, 2px right, the glyph at 140% of the editor font size.
        let icon = self.icon(name, metrics.line_height, 1.4 * metrics.font_size, Rgba { a: color.a * opacity, ..color });
        lane = lane.child(div().w(px(DECORATIONS_WIDTH)).ml(px(2.)).h(px(metrics.line_height)).flex().justify_center().cursor_pointer().child(icon));
        lane
    }

    /// Whether `visible` falls at or after the end of `line`, as a click on a collapsed region's ellipsis does.
    fn at_line_end(&self, line: usize, visible: f32) -> bool {
        let editor = self.editors.active().expect("text is clickable only with an editor open");
        let text = &editor.document.lines[line];
        editor.folding.has_hidden() && column_at_visible(text, visible, editor.document.indentation.tab_size) == text.chars().count()
    }

    /// folding.ts `onEditorMouseUp`: a release on the line pressed toggles its region, with Alt the regions around
    /// it, with Shift or the middle button the ones inside it first; then the line's start is revealed.
    fn release_fold_click(&mut self, line: usize, icon: bool, modifiers: gpui::Modifiers, middle: bool, cx: &mut Context<Self>) {
        if self.fold_mouse_down.take() != Some((line, icon)) {
            return;
        }
        let editor = self.editors.active_mut().expect("folding controls show only with an editor open");
        let before = (editor.selections.clone(), editor.folding.hidden().to_vec());
        if editor.click_fold_control(line, icon, modifiers.alt, middle || modifiers.shift) {
            self.cursors_changed(before.0, Reveal::CenterIfOutside(Position::new(line, 0)), "mouse", cx);
            self.folds_changed(&before.1);
        }
    }

    /// The view lines changed with the folds: the host hears the new visible ranges (`onDidScrollChange` fires
    /// for the new scroll height).
    fn folds_changed(&mut self, hidden_before: &[Range<usize>]) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("folds change in the active editor");
        if editor.folding.hidden() != hidden_before {
            editor.set_scroll(editor.scroll.top, editor.scroll.left, viewport);
            self.host.send_visible_ranges(&self.editors);
        }
    }

    /// bracketMatching.ts `_recomputeBrackets`: the brackets matched at each empty cursor, none past 100 selections.
    fn bracket_matches(&self, editor: &OpenEditor) -> Vec<([BracketRange; 2], bool)> {
        if editor.selections.len() > 100 {
            return Vec::new();
        }
        let lines = &editor.document.lines;
        let mut matches = Vec::new();
        for selection in editor.selections.iter().filter(|selection| selection.is_empty()) {
            matches.extend(editor.document.tokens.bracket_match(&self.theme, &self.languages, lines, (selection.head.line, selection.head.column)));
        }
        matches
    }

    /// viewController.ts `dispatchMouse` on the text: a click places the cursor, Shift or a drag extends the
    /// selection, a double click selects the word and a triple click the line. With Alt (`multiCursorModifier`)
    /// a click adds or removes a cursor, a drag extends the cursor added last, a double or triple click
    /// selects its word or line, and with Shift too it selects a column.
    /// shortcut: a drag selects only while the pointer is over the lines, and by chars, not by the words or lines it started with.
    /// longLinesHelper.ts: a press on the text at or past the cut renders every line whole from then on.
    fn show_long_line(&mut self, line: usize, visible: f32, char_width: f32, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("text is clickable only with an editor open");
        let text = &editor.document.lines[line];
        let Some((limit, _, label)) = line_overflow(text, self.stop_rendering_line_after) else { return };
        let tab_size = editor.document.indentation.tab_size;
        // The "Show more" span is text too; a press right of it is empty content.
        let span_end = visible_column(text, limit, tab_size) as f32 + show_more_width(&label, char_width) / char_width;
        if column_at_visible(text, visible, tab_size) + 1 >= limit && visible < span_end {
            self.stop_rendering_line_after = None;
            cx.notify();
        }
    }

    fn click_text(&mut self, line: usize, visible: f32, modifiers: gpui::Modifiers, select: bool, click_count: usize, cx: &mut Context<Self>) {
        let column_selection = self.setting("editor.columnSelection") == json!(true);
        // viewController.ts `_hasMulticursorModifier` and `_hasNonMulticursorModifier`; `ctrlCmd` is Cmd on macOS.
        let (multi_cursor, non_multi_cursor) = match self.multi_cursor_modifier_is_cmd() {
            true => (modifiers.platform, modifiers.control || modifiers.alt),
            false => (modifiers.alt, modifiers.control || modifiers.platform),
        };
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let text = &editor.document.lines[line];
        let tab_size = editor.document.indentation.tab_size;
        let column = match line_overflow(text, self.stop_rendering_line_after) {
            // The reference puts a press on the "Show more" span before the last drawn char.
            Some((limit, _, _)) if visible >= visible_column(text, limit, tab_size) as f32 => limit - 1,
            _ => column_at_visible(text, visible, tab_size),
        };
        let position = Position::new(line, column);
        let before = editor.selections.clone();
        if multi_cursor && click_count < 4 {
            if click_count == 1 && non_multi_cursor {
                return;
            }
            // Only the column selection reveals; the other `_lastCursor*` commands and `createCursor` do not.
            let reveal = match click_count {
                2 => {
                    editor.last_cursor_select_word(position);
                    Reveal::Nothing
                }
                3 => {
                    editor.last_cursor_select_line(line);
                    Reveal::Nothing
                }
                _ if modifiers.shift => editor.column_select_to_pointer(line, mouse_column(visible), true),
                _ if select => {
                    editor.last_cursor_select_to(position);
                    Reveal::Nothing
                }
                _ => {
                    editor.toggle_cursor(position);
                    Reveal::Nothing
                }
            };
            self.cursors_changed(before, reveal, "mouse", cx);
            return;
        }
        match click_count {
            2 => {
                let word = word_at(text, column).unwrap_or(column..column);
                editor.move_to(Position::new(line, word.start), false);
                editor.move_to(Position::new(line, word.end), true);
            }
            3.. => {
                let end = if line + 1 < editor.document.lines.len() { Position::new(line + 1, 0) } else { Position::new(line, text.chars().count()) };
                editor.move_to(Position::new(line, 0), false);
                editor.move_to(end, true);
            }
            _ if select && (column_selection || modifiers.alt) => {
                let reveal = editor.column_select_to_pointer(line, mouse_column(visible), true);
                self.cursors_changed(before, reveal, "mouse", cx);
                return;
            }
            _ => editor.move_to(Position::new(line, column), select),
        }
        let moved = editor.selections != before;
        self.editor_edited(None, moved, "mouse", cx);
    }

    /// `editor.multiCursorModifier`: any value but `ctrlCmd` is Alt (editorOptions.ts `_multiCursorModifierFromString`).
    pub(super) fn multi_cursor_modifier_is_cmd(&self) -> bool {
        self.setting("editor.multiCursorModifier") == "ctrlCmd"
    }

    /// The middle button selects a column (`editor.scrollOnMiddleClick` off): from the pointer, or with
    /// Shift or while dragging from where the last column selection started.
    fn column_select_text(&mut self, line: usize, visible: f32, extend: bool, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let before = editor.selections.clone();
        let reveal = editor.column_select_to_pointer(line, mouse_column(visible), extend);
        self.cursors_changed(before, reveal, "mouse", cx);
    }

    /// Runs an editor command; false when its precondition does not hold.
    pub(super) fn run_editor_command(&mut self, command: &str, source: &str, cx: &mut Context<Self>) -> bool {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("editor commands run on the active editor");
        let before = editor.selections.clone();
        let hidden_before = editor.folding.hidden().to_vec();
        let Some(reveal) = commands::run(editor, command, viewport) else {
            return false;
        };
        self.cursors_changed(before, reveal, source, cx);
        self.folds_changed(&hidden_before);
        true
    }

    /// After a command that moves cursors only: scrolls to what it reveals and tells the host.
    pub(super) fn cursors_changed(&mut self, before: Vec<Selection>, reveal: Reveal, source: &str, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("cursors change in the active editor");
        let scroll = editor.scroll;
        match reveal {
            Reveal::Nothing => {}
            Reveal::All => editor.reveal_cursors(source, viewport),
            Reveal::Position(position) => editor.reveal_position(position, source, viewport),
            Reveal::CenterIfOutside(position) => editor.reveal_range(position, position, VerticalReveal::CenterIfOutsideViewport, viewport),
        }
        let (moved, scrolled) = (editor.selections != before, editor.scroll != scroll);
        if moved {
            self.find_editor_edited(None);
            self.host.send_selection(self.editors.active().expect("still open"), source);
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// decorationsOverviewRuler.ts `_render`, in device pixels: word highlights, matched brackets and find matches in the center lane, the cursor
    /// across all lanes, then the 1px border on the left and top edges.
    fn overview_ruler(&self, editor: &OpenEditor) -> gpui::Canvas<()> {
        let line_count = editor.document.lines.len();
        let highlight_lines: Vec<(usize, usize)> = editor.highlights.iter().map(|(line, _)| (*line, *line)).collect();
        let highlight = self.color("editorOverviewRuler.wordHighlightTextForeground");
        let mut bracket_lines = Vec::new();
        for ([open, close], in_ruler) in self.bracket_matches(editor) {
            if in_ruler {
                bracket_lines.extend([(open.0, open.0), (close.0, close.0)]);
            }
        }
        let bracket_color = self.color("editorOverviewRuler.bracketMatchForeground");
        // decorationsOverviewRuler.ts onCursorStateChanged: the cursor colors, each at 70% opacity.
        let ruler_color = |key: &str| self.theme.color(key).map(|color| Rgba { a: color.a * 0.7, ..color });
        let mut cursors = Vec::new();
        for (index, selection) in editor.selections.iter().enumerate() {
            let key = match (editor.selections.len(), index) {
                (1, _) => "editorCursor.foreground",
                (_, 0) => "editorMultiCursor.primary.foreground",
                _ => "editorMultiCursor.secondary.foreground",
            };
            cursors.push((selection.head.line, ruler_color(key)));
        }
        cursors.sort_by_key(|(line, _)| *line);
        bracket_lines.sort();
        let find_color = self.color("editorOverviewRuler.findMatchForeground");
        let range_lines: Vec<(usize, usize)> = self.quick_access.goto_line.iter().filter_map(|state| state.highlight(editor.model_id)).map(|position| (position.line, position.line)).collect();
        let range_color = self.color("editorOverviewRuler.rangeHighlightForeground");
        let (mut find_lines, mut current_find_lines) = (Vec::new(), Vec::new());
        for mark in self.find_marks(editor, self.editors.viewport.height) {
            let lines = if mark.current { &mut current_find_lines } else { &mut find_lines };
            lines.push((mark.range.start.line, mark.range.end.line));
        }
        let border = self.theme.color("editorOverviewRuler.border");
        let Viewport { top_whitespace, line_height, .. } = self.editors.viewport;
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
                let scroll_height = top_whitespace + line_count as f32 * line_height + viewport_height - line_height;
                let ratio = canvas_height / scroll_height;
                let lanes = RulerLanes::new(canvas_width);
                let min_height = (6. * scale).floor();
                // In z-index order: Go to Line's range in all lanes, find matches at 10, the current match at 13.
                let center = (lanes.center_x, lanes.center_width);
                for (lines, color, (x, width)) in [
                    (&range_lines, range_color, (lanes.full_x, lanes.full_width)),
                    (&highlight_lines, highlight, center),
                    (&bracket_lines, bracket_color, center),
                    (&find_lines, find_color, center),
                    (&current_find_lines, find_color, center),
                ] {
                    let mut previous: Option<(f32, f32)> = None;
                    for (start_line, end_line) in lines {
                        let mut y1 = ((top_whitespace + *start_line as f32 * line_height) * ratio).floor();
                        let mut y2 = ((top_whitespace + *end_line as f32 * line_height + line_height) * ratio).floor();
                        if y2 - y1 < min_height {
                            let half = (min_height / 2.).floor();
                            let center = ((y1 + y2) / 2.).floor().clamp(half, canvas_height - half);
                            (y1, y2) = (center - half, center + half);
                        }
                        previous = match previous {
                            Some((start, end)) if y1 <= end + 1. => Some((start, end.max(y2))),
                            Some((start, end)) => {
                                paint(window, x, start, width, end - start, color);
                                Some((y1, y2))
                            }
                            None => Some((y1, y2)),
                        };
                    }
                    if let Some((start, end)) = previous {
                        paint(window, x, start, width, end - start, color);
                    }
                }
                // Cursors sorted by line; touching marks of one color paint as one.
                let height = (2. * scale).floor();
                let half = (height / 2.).floor();
                let mut previous: Option<(f32, f32, Rgba)> = None;
                for (line, color) in cursors.iter().filter_map(|(line, color)| Some((*line, (*color)?))) {
                    let y1 = ((top_whitespace + line as f32 * line_height) * ratio).floor().clamp(half, canvas_height - half) - half;
                    let y2 = y1 + height;
                    previous = match previous {
                        Some((start, end, previous_color)) if y1 <= end + 1. && previous_color == color => Some((start, end.max(y2), color)),
                        Some((start, end, previous_color)) => {
                            paint(window, lanes.full_x, start, lanes.full_width, end - start, previous_color);
                            Some((y1, y2, color))
                        }
                        None => Some((y1, y2, color)),
                    };
                }
                if let Some((start, end, color)) = previous {
                    paint(window, lanes.full_x, start, lanes.full_width, end - start, color);
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

    /// The column selection, tab focus and overtype mode items, then with an editor open its selection, indentation, encoding, end of line and
    /// language items (editorStatus.ts), right to left before the notifications bell.
    pub(super) fn editor_status_items(&self, foreground: Rgba, window: &Window, cx: &mut Context<Self>) -> Div {
        // editorStatus.ts `updateColumnSelectionModeElement`, `updateTabFocusModeElement` and `updateInputModeElement`: prominent entries.
        let mut items = Vec::new();
        if self.editors.active().is_some() && self.setting("editor.columnSelection") == true {
            items.push(("Column Selection".to_string(), "Disable Column Selection Mode", "editor.action.toggleColumnSelection"));
        }
        if self.tab_focus_mode() {
            items.push(("Tab Moves Focus".to_string(), "Disable Accessibility Mode", "editor.action.toggleTabFocusMode"));
        }
        if self.overtype {
            items.push(("OVR".to_string(), "Enable Insert Mode", "editor.action.toggleOvertypeInsertMode"));
        }
        let prominent_items = items.len();
        if let Some(editor) = self.editors.active() {
            items.extend(Self::editor_status_entries(editor, &self.languages));
        }
        let prominent = (self.theme.color("statusBarItem.prominentBackground"), self.color("statusBarItem.prominentForeground"));
        // GPUI rounds a text box up to a whole px, Chrome keeps the fraction: the shaped width keeps the items in place.
        let mut style = window.text_style();
        style.font_features = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
        let text_width = |label: &str| window.text_system().shape_line(label.to_owned().into(), px(12.), &[style.to_run(label.len())], None).width;
        let items = items.into_iter().enumerate().map(|(index, (label, tooltip, command))| {
            let hover = self.hover_target(tooltip, super::hover::HoverContent::Text(tooltip.into()), super::hover::HoverOptions::status_bar());
            let item = div()
                .id(("editor-status", index))
                .relative()
                .h_full()
                .w(text_width(&label) + px(10.))
                .flex_none()
                .mx(px(3.))
                .px(px(5.))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .cursor_pointer()
                .child(label)
                .child(hover)
                // statusbarItem.ts executeCommand on click.
                .on_click(cx.listener(move |workbench, _, window, cx| {
                    let outcome = workbench.execute_command(command, Vec::new(), window, cx);
                    super::quick_access::show_failure(command.to_owned(), outcome, window, cx);
                }));
            if index >= prominent_items {
                return item.into_any_element();
            }
            // `.prominent-kind` colors the container around the label's 3px margins, with modernUI statusBar.css
            // cornerRadius.small (4px).
            div().h_full().flex_none().rounded(px(4.)).text_color(prominent.1).when_some(prominent.0, |container, background| container.bg(background)).child(item).into_any_element()
        });
        div().h_full().flex().text_color(foreground).children(items)
    }

    /// editorStatus.ts entries of the editor: text, tooltip and command.
    fn editor_status_entries(editor: &OpenEditor, languages: &crate::languages::Languages) -> [(String, &'static str, &'static str); 5] {
        let document = &editor.document;
        let Indentation { tab_size, insert_spaces, .. } = document.indentation;
        let indent_size = document.indentation.indent_size();
        let indentation = match (insert_spaces, tab_size == indent_size) {
            (true, true) => format!("Spaces: {indent_size}"),
            (true, false) => format!("Spaces: {indent_size} (Tab Size: {tab_size})"),
            (false, _) => format!("Tab Size: {tab_size}"),
        };
        let encoding = SUPPORTED_ENCODINGS.iter().find(|supported| supported.id == document.encoding).map_or(document.encoding.as_str(), |supported| supported.label_short);
        let eol = if document.eol == "\r\n" { "CRLF" } else { "LF" };
        [
            (selection_label(editor), "Go to Line/Column", "workbench.action.gotoLine"),
            (indentation, "Select Indentation", super::editor_status::CHANGE_INDENTATION),
            (encoding.into(), "Select Encoding", "workbench.action.editor.changeEncoding"),
            (eol.into(), "Select End of Line Sequence", "workbench.action.editor.changeEOL"),
            (languages.name(&document.language_id).to_string(), "Select Language Mode", "workbench.action.editor.changeLanguageMode"),
        ]
    }
}

#[derive(Clone, Copy)]
struct EditorMetrics {
    font_size: f32,
    line_height: f32,
    char_width: f32,
    line_numbers_width: f32,
}

impl EditorMetrics {
    /// One glyph lane as wide as a line is high (editorOptions.ts layout).
    fn glyph_margin_width(&self) -> f32 {
        self.line_height
    }

    fn content_left(&self) -> f32 {
        self.glyph_margin_width() + self.line_numbers_width + DECORATIONS_WIDTH
    }

    /// fontInfo.ts `BareFontInfo._create`: the editor zoom scales the font size and line height; then
    /// `typicalHalfwidthCharacterWidth` and `maxDigitWidth` of that font. The line numbers are as wide as the
    /// longest number, at least lineNumbersMinChars digits, rounded (editorOptions.ts).
    fn measure(window: &Window, line_count: usize, zoom: f32) -> Self {
        let (font_size, line_height) = (EDITOR_FONT_SIZE * zoom, LINE_HEIGHT * zoom);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font(EDITOR_FONT));
        let advance = |c: char| f32::from(text_system.advance(font_id, px(font_size), c).expect("Menlo has ASCII glyphs").width);
        let max_digit_width = ('0'..='9').map(advance).fold(0., f32::max);
        let digits = line_count.to_string().len().max(LINE_NUMBERS_MIN_CHARS);
        EditorMetrics { font_size, line_height, char_width: advance('x'), line_numbers_width: (digits as f32 * max_digit_width).round() }
    }
}

struct LineColors {
    number: Rgba,
    active_number: Rgba,
    text: Rgba,
    line_border: Rgba,
    word: Option<Rgba>,
    word_border: Option<Rgba>,
    selection_highlight: Option<Rgba>,
    selection_highlight_border: Option<Rgba>,
    guide: Rgba,
    cursor: Rgba,
    primary_cursor: Rgba,
    secondary_cursor: Rgba,
    selection: Rgba,
}

/// editorStatus.ts `getSelectionLabel`: the primary cursor's line and visible column for one cursor, the
/// count for several, and the chars selected (`getCharacterCountInRange`: code points, a line break as the EOL's length).
fn selection_label(editor: &OpenEditor) -> String {
    let document = &editor.document;
    let mut selected = 0;
    for selection in &editor.selections {
        let (start, end) = (selection.start(), selection.end());
        for line in start.line..=end.line {
            let first = if line == start.line { start.column } else { 0 };
            let last = if line == end.line { end.column } else { document.lines[line].chars().count() };
            selected += last - first;
        }
        selected += document.eol.len() * (end.line - start.line);
    }
    if let [selection] = editor.selections.as_slice() {
        let Position { line, column } = selection.head;
        let visible = visible_column(&document.lines[line], column, document.indentation.tab_size);
        return match selected {
            0 => format!("Ln {}, Col {}", line + 1, visible + 1),
            _ => format!("Ln {}, Col {} ({selected} selected)", line + 1, visible + 1),
        };
    }
    match selected {
        0 => format!("{} selections", editor.selections.len()),
        _ => format!("{} selections ({selected} characters selected)", editor.selections.len()),
    }
}

const ROUNDED_PIECE_WIDTH: f32 = 10.;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Corner {
    Extern,
    Intern,
    Flat,
}

#[derive(PartialEq, Debug)]
struct SelectionCorners {
    start_top: Corner,
    start_bottom: Corner,
    end_top: Corner,
    end_bottom: Corner,
}

/// selections.ts `_enrichVisibleRangesWithStyle` for one line's range against the same selection's range on
/// the view lines above and below. Upstream keeps the previous frame's corners at the viewport edges; the
/// neighbor lines past the edge give the same corners.
fn selection_corners(above: Option<(f32, f32)>, (left, right): (f32, f32), below: Option<(f32, f32)>, epsilon: f32) -> SelectionCorners {
    let mut corners = SelectionCorners { start_top: Corner::Extern, start_bottom: Corner::Extern, end_top: Corner::Extern, end_bottom: Corner::Extern };
    if let Some((above_left, above_right)) = above {
        if (left - above_left).abs() < epsilon {
            corners.start_top = Corner::Flat;
        } else if left > above_left {
            corners.start_top = Corner::Intern;
        }
        if (right - above_right).abs() < epsilon {
            corners.end_top = Corner::Flat;
        } else if above_left < right && right < above_right {
            corners.end_top = Corner::Intern;
        }
    }
    if let Some((below_left, below_right)) = below {
        if (left - below_left).abs() < epsilon {
            corners.start_bottom = Corner::Flat;
        } else if below_left < left && left < below_right {
            corners.start_bottom = Corner::Intern;
        }
        if (right - below_right).abs() < epsilon {
            corners.end_bottom = Corner::Flat;
        } else if right < below_right {
            corners.end_bottom = Corner::Intern;
        }
    }
    corners
}

/// The selected chars of a line and whether its line break is selected too.
/// whitespace.ts with `experimentalWhitespaceRendering: svg`: a circle for a space, an arrow for a tab, each at
/// its mark's left edge on the line.
fn whitespace_canvas(marks: Vec<(f32, bool)>, space_width: f32, line_height: f32, color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center_y = line_height / 2.;
            let at = |x: f32, y: f32| bounds.origin + point(px(x), px(y));
            for (left, tab) in &marks {
                if !tab {
                    let radius = (space_width / 7. * 100.).round() / 100.;
                    let center_x = left + space_width / 2.;
                    let circle = Bounds::new(at(center_x - radius, center_y - radius), size(px(radius * 2.), px(radius * 2.)));
                    window.paint_quad(fill(circle, color).corner_radii(px(radius)));
                    continue;
                }
                // _renderArrow: a shaft half a stroke thick either side of the middle, then the head.
                let stroke = space_width / 7.;
                let tip = 100. / 125. * space_width;
                let p1 = (0., stroke / 2.);
                let p2 = (tip, p1.1);
                let p3 = (p2.0 - 0.2 * tip, p2.1 + 0.2 * tip);
                let p4 = (p3.0 + 0.1 * tip, p3.1 + 0.1 * tip);
                let p5 = (p4.0 + 0.35 * tip, p4.1 - 0.35 * tip);
                let points = [p1, p2, p3, p4, p5, (p5.0, -p5.1), (p4.0, -p4.1), (p3.0, -p3.1), (p2.0, -p2.1), (p1.0, -p1.1)];
                let mut arrow = PathBuilder::fill();
                arrow.move_to(at(left + p1.0, center_y + p1.1));
                for (x, y) in &points[1..] {
                    arrow.line_to(at(left + x, center_y + y));
                }
                arrow.close();
                if let Ok(path) = arrow.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(super) fn selected_columns(selection: Selection, line: usize, text: &str) -> Option<(usize, usize, bool)> {
    let (start, end) = (selection.start(), selection.end());
    if selection.is_empty() || line < start.line || line > end.line {
        return None;
    }
    let first = if line == start.line { start.column } else { 0 };
    let last = if line == end.line { end.column } else { text.chars().count() };
    Some((first, last, line < end.line))
}

/// mouseTarget.ts `mouseColumn` less one: the visible column nearest the pointer, past the line end too.
fn mouse_column(visible: f32) -> usize {
    visible.round().max(0.) as usize
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



/// scrollbarVisibilityController.ts for both scrollbars, with the CSS opacity transition between its states.
#[derive(Default)]
pub(super) struct Scrollbars {
    pub mouse_over: bool,
    shown: bool,
    /// When `shown` last flipped, and the opacity the transition started from.
    changed: Option<(Instant, f32)>,
    hide_timer: Option<Task<()>>,
    drag: Option<SliderDrag>,
}

impl Scrollbars {
    fn set_shown(&mut self, shown: bool, now: Instant) {
        if self.shown != shown {
            let from = self.opacity(now);
            self.shown = shown;
            self.changed = Some((now, from));
        }
    }

    /// Linear from where the last change started: 100ms to visible, 800ms to invisible.
    fn opacity(&self, now: Instant) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        let Some((at, from)) = self.changed else { return target };
        let duration = if self.shown { SCROLLBAR_FADE_IN } else { SCROLLBAR_FADE_OUT };
        let progress = (now.duration_since(at).as_secs_f32() / duration.as_secs_f32()).min(1.);
        from + (target - from) * progress
    }

    fn in_transition(&self, now: Instant) -> bool {
        self.opacity(now) != if self.shown { 1. } else { 0. }
    }
}

/// folding.css: the expanded folding controls fade with `transition: opacity 0.5s` (ease) as the pointer enters
/// or leaves the margin.
#[derive(Default)]
pub(super) struct FoldControls {
    shown: bool,
    changed: Option<(Instant, f32)>,
}

impl FoldControls {
    fn set_shown(&mut self, shown: bool, now: Instant) {
        if self.shown != shown {
            let from = self.opacity(now);
            self.shown = shown;
            self.changed = Some((now, from));
        }
    }

    fn opacity(&self, now: Instant) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        let Some((at, from)) = self.changed else { return target };
        let progress = (now.duration_since(at).as_secs_f32() / FOLD_CONTROLS_FADE.as_secs_f32()).min(1.);
        from + (target - from) * ease(progress)
    }

    fn in_transition(&self, now: Instant) -> bool {
        self.opacity(now) != if self.shown { 1. } else { 0. }
    }
}

/// CSS `ease`, cubic-bezier(0.25, 0.1, 0.25, 1): the curve's x solved by bisection, then its y.
fn ease(x: f32) -> f32 {
    let bezier = |p1: f32, p2: f32, t: f32| 3. * p1 * t * (1. - t).powi(2) + 3. * p2 * t * t * (1. - t) + t.powi(3);
    let (mut low, mut high) = (0., 1.);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if bezier(0.25, 0.25, mid) < x { low = mid } else { high = mid }
    }
    bezier(0.1, 1., (low + high) / 2.)
}

#[derive(Clone, Copy)]
struct SliderDrag {
    axis: Axis,
    /// The pointer's window coordinate along the axis at the press, and the scrollbar then.
    pointer: f32,
    state: Scrollbar,
}

fn scrollbar(editor: &OpenEditor, axis: Axis, viewport: Viewport) -> Scrollbar {
    match axis {
        Axis::Vertical => editor.vertical_scrollbar(viewport),
        Axis::Horizontal => editor.horizontal_scrollbar(viewport),
    }
}

fn set_axis_scroll(editor: &mut OpenEditor, axis: Axis, position: f32, viewport: Viewport) -> bool {
    match axis {
        Axis::Vertical => editor.set_scroll(position, editor.scroll.left, viewport),
        Axis::Horizontal => editor.set_scroll(editor.scroll.top, position, viewport),
    }
}

/// The slider spans the scrollbar's thickness (`verticalSliderSize` and `horizontalSliderSize` default to it).
fn slider_bounds(track: Bounds<Pixels>, axis: Axis, bar: Scrollbar) -> Bounds<Pixels> {
    match axis {
        Axis::Vertical => Bounds::new(track.origin + point(px(0.), px(bar.slider_position)), size(track.size.width, px(bar.slider_size))),
        Axis::Horizontal => Bounds::new(track.origin + point(px(bar.slider_position), px(0.)), size(px(bar.slider_size), track.size.height)),
    }
}

/// The complementary error function (Abramowitz and Stegun 7.1.26, error below 1.5e-7).
pub(super) fn erfc(x: f32) -> f32 {
    let t = 1. / (1. + 0.327_591_1 * x);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    poly * (-x * x).exp()
}

#[cfg(test)]
mod tests {
    use super::{Corner, SelectionCorners, selection_corners};
    use Corner::{Extern, Flat, Intern};

    #[test]
    fn selection_corners_follow_the_neighbor_lines() {
        let corners = |start_top, start_bottom, end_top, end_bottom| SelectionCorners { start_top, start_bottom, end_top, end_bottom };
        // One line: every corner rounds out.
        assert_eq!(selection_corners(None, (10., 50.), None, 2.), corners(Extern, Extern, Extern, Extern));
        // A middle line starting left of the line above, ending left of the line below.
        assert_eq!(selection_corners(Some((20., 80.)), (0., 80.), Some((0., 100.)), 2.), corners(Extern, Flat, Flat, Intern));
        // A last line starting right of the line above and ending inside it.
        assert_eq!(selection_corners(Some((0., 80.)), (10., 30.), None, 2.), corners(Intern, Extern, Intern, Extern));
        // A start within epsilon of the line below counts as aligned.
        assert_eq!(selection_corners(None, (20.5, 80.), Some((20., 60.)), 2.), corners(Extern, Flat, Extern, Extern));
    }
}
