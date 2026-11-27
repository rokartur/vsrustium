//! The workbench hover: hoverService.ts `setupManagedHover` and `setupDelayedHover` showing a hoverWidget.ts
//! `HoverWidget` in a context view, placed by its `layout` and drawn as hover.css, hoverWidget.css and the
//! Modern UI frosted glass draw it. Targets add `hover_target` as a child; the editor hover reuses the widget.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, BoxShadow, Context, DispatchPhase, ElementId, HitboxBehavior, Hsla, MouseDownEvent, MouseMoveEvent, PathBuilder, Pixels, Point, Size, Task,
    TextRun, Window, canvas, div, point, prelude::*, px, rgba,
};

use super::Workbench;
use super::context_menu::{LayoutAnchorPosition, layout2d};
use crate::notifications::{Node, parse_linked_text};

#[cfg(test)]
mod tests;

/// hoverWidget.ts `Constants`.
const POINTER_SIZE: f32 = 3.;
const HOVER_BORDER_WIDTH: f32 = 2.;
const WINDOW_EDGE_MARGIN: f32 = 2.;
/// `.compact`: 12px text, 19px lines, 2px 8px padding (hover.css).
pub(super) const FONT_SIZE: f32 = 12.;
pub(super) const LINE_HEIGHT: f32 = 19.;
const PADDING_X: f32 = 8.;
const PADDING_Y: f32 = 2.;
const MAX_WIDTH: f32 = 700.;
const TEXT_MAX_WIDTH: f32 = 500.;
const PARAGRAPH_GAP: f32 = 8.;
/// CompositeMouseTracker's `_eventDebounceDelay` and WorkbenchHoverDelegate's `timeLimit`.
const MOUSE_OUT_DELAY: Duration = Duration::from_millis(200);
/// `.with-pointer` has 3px corners; roundedCorners.css gives other hovers cornerRadius.large.
const POINTER_RADIUS: f32 = 3.;
const RADIUS: f32 = 8.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HoverPosition {
    Left,
    Right,
    Below,
    Above,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HoverContent {
    Text(String),
    /// An action's tooltip: its title and the keybinding of its command, looked up when the hover shows.
    Action { title: &'static str, command: &'static str },
    /// editorTabsControl.ts `getHoverTitle` of a preview editor: the title, then "(_preview_ $(gear))".
    Preview(String),
    /// A trusted markdown tooltip: paragraphs split by a blank line, each linked text (`[label](href)`).
    Markdown(String),
    /// languageStatus.ts `_combinedEntryTooltip`: one row per status of the current view model.
    LanguageStatus,
}

/// What the target's hover delegate decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverOptions {
    position: HoverPosition,
    /// 'mouse' placement: 10px right of the pointer, without a pointer caret.
    follows_mouse: bool,
    /// `instantHover`: within 200ms of hiding a hover the next one shows without delay.
    instant: bool,
    /// `setupDelayedHover`'s `groupId`: moving to a target of the showing hover's group shows at once.
    group: Option<&'static str>,
    max_height_ratio: f32,
}

impl HoverOptions {
    /// getDefaultHoverDelegate('mouse'): tabs and Explorer rows.
    pub fn mouse() -> Self {
        HoverOptions { position: HoverPosition::Below, follows_mouse: true, instant: false, group: None, max_height_ratio: 0.5 }
    }

    /// createInstantHoverDelegate: the title bar's actions and command center.
    pub fn instant() -> Self {
        HoverOptions { position: HoverPosition::Above, follows_mouse: false, instant: true, group: None, max_height_ratio: 0.5 }
    }

    /// statusbarPart.ts's WorkbenchHoverDelegate, with `maxHeightRatio: 0.9`.
    pub fn status_bar() -> Self {
        HoverOptions { max_height_ratio: 0.9, ..Self::instant() }
    }

    /// getDefaultHoverDelegate('element'): above the target, after the hover delay.
    pub fn element() -> Self {
        HoverOptions { instant: false, ..Self::instant() }
    }

    /// updatableHoverWidget.ts: an action item's managed hover asks for `HoverPosition.BELOW`.
    pub fn managed() -> Self {
        HoverOptions { position: HoverPosition::Below, ..Self::instant() }
    }

    /// A pointer on an action item: its managed hover below the element, after the default element delegate's delay.
    pub fn action_item() -> Self {
        HoverOptions { instant: false, ..Self::managed() }
    }

    /// `showInstantHover` with `HoverPosition.BELOW` and `showPointer`: the profile icon picker.
    pub fn sticky_below() -> Self {
        HoverOptions { position: HoverPosition::Below, ..Self::instant() }
    }

    /// compositeBarActions.ts: `HoverStyle.Pointer` on the side away from the side bar.
    pub fn composite_bar(position: HoverPosition) -> Self {
        HoverOptions { position, follows_mouse: false, instant: false, group: Some("composite-bar-actions"), max_height_ratio: 0.5 }
    }
}

#[derive(Clone)]
struct HoverTarget {
    id: ElementId,
    content: HoverContent,
    options: HoverOptions,
    bounds: Bounds<Pixels>,
    /// `target.x` of a mouse placed hover: the last pointer x plus 10; none for a hover shown from the keyboard.
    mouse_x: Option<f32>,
}

struct ShownHover {
    target: HoverTarget,
    text: String,
    fade_in: bool,
    serial: usize,
}

#[derive(Default)]
pub struct Hovers {
    /// The target under the pointer, its hover pending or shown.
    target: Option<HoverTarget>,
    /// A click or a key hid the target's hover; it shows again only after the pointer leaves the target.
    dismissed: bool,
    shown: Option<ShownHover>,
    shown_count: usize,
    pending: Option<Task<()>>,
    hiding: Option<Task<()>>,
    /// WorkbenchHoverDelegate's `lastHoverHideTime`.
    last_hide: Option<Instant>,
    /// `HoverWidget.isLocked`: Alt is held, so the hover stays when the pointer leaves for it.
    locked: bool,
    /// `HoverWidget.isMouseIn`.
    mouse_in_hover: bool,
}

impl Hovers {
    #[cfg(test)]
    pub fn shown_text(&self) -> Option<&str> {
        self.shown.as_ref().map(|hover| hover.text.as_str())
    }
}

impl Workbench {
    /// `setupManagedHover`: an overlay over its parent that shows the hover while the pointer rests on the parent.
    pub(super) fn hover_target(&self, id: impl Into<ElementId>, content: HoverContent, options: HoverOptions) -> impl IntoElement {
        let id = id.into();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                let target = HoverTarget { id: id.clone(), content: content.clone(), options, bounds, mouse_x: None };
                let pressed = hitbox.clone();
                // Capture phase, as the reference's listeners: a target's own handlers may stop the event.
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some(Some(workbench)) = window.root::<Workbench>() else { return };
                    let hovered = hitbox.is_hovered(window);
                    if !hovered && !workbench.read(cx).hovers.is_target(&target.id) {
                        return;
                    }
                    workbench.update(cx, |workbench, cx| workbench.hover_mouse_moved(&target, hovered, event.position, window, cx));
                });
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !pressed.is_hovered(window) {
                        return;
                    }
                    let Some(Some(workbench)) = window.root::<Workbench>() else { return };
                    workbench.update(cx, |workbench, cx| workbench.dismiss_hover(cx));
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// An action view item's hover: its title, and its keybinding when it has one.
    pub(super) fn action_hover(&self, title: &'static str, command: &'static str, options: HoverOptions) -> impl IntoElement {
        self.hover_target(command, HoverContent::Action { title, command }, options)
    }

    fn hover_mouse_moved(&mut self, target: &HoverTarget, hovered: bool, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        // setupManagedHover's `x: e.x + 10`; MouseEvent.x is whole pixels.
        let mouse_x = f32::from(position.x).floor() + 10.;
        if !hovered {
            self.leave_hover_target(cx);
            return;
        }
        if let Some(current) = self.hovers.target.as_mut().filter(|current| current.id == target.id) {
            current.mouse_x = Some(mouse_x);
            return;
        }
        // Entering a target leaves the previous one first, whichever target's handler runs first.
        self.leave_hover_target(cx);
        let target = HoverTarget { mouse_x: Some(mouse_x), ..target.clone() };
        // Back on the target within the mouse tracker's delay: the hover stays.
        if self.hovers.shown.as_ref().is_some_and(|shown| shown.target.id == target.id) {
            self.hovers.hiding = None;
            self.hovers.target = Some(target);
            return;
        }
        let instantly = target.options.instant && self.hovers.last_hide.is_some_and(|hidden| cx.background_executor().now() - hidden < MOUSE_OUT_DELAY);
        let same_group = target.options.group.is_some() && self.hovers.shown.as_ref().is_some_and(|shown| shown.target.options.group == target.options.group);
        self.hovers.target = Some(target);
        if instantly || same_group {
            self.show_hover(false, window, cx);
            return;
        }
        let delay = Duration::from_millis(self.setting("workbench.hover.delay").as_u64().unwrap_or(1500));
        self.hovers.pending = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.show_hover(true, window, cx));
        }));
    }

    /// The pointer left the target: a pending hover is cancelled, a shown one hides once the mouse tracker's delay passes.
    pub(super) fn leave_hover_target(&mut self, cx: &mut Context<Self>) {
        if self.hovers.target.take().is_none() {
            return;
        }
        self.hovers.pending = None;
        self.hovers.dismissed = false;
        self.hide_hover_later(cx);
    }

    /// CompositeMouseTracker's `onMouseOut`: the hover hides once the pointer stays out of the target, and of the
    /// hover when it has a link, for 200ms.
    fn hide_hover_later(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.hovers.shown.as_ref() else { return };
        self.hovers.last_hide = Some(cx.background_executor().now());
        let serial = shown.serial;
        self.hovers.hiding = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(MOUSE_OUT_DELAY).await;
            let _ = workbench.update(cx, |workbench, cx| {
                if !workbench.hovers.locked && workbench.hovers.shown.as_ref().is_some_and(|shown| shown.serial == serial) {
                    workbench.hovers.shown = None;
                    cx.notify();
                }
            });
        }));
    }

    /// hoverService.ts `showManagedHover` (`list.showHover`): the target's hover at once, placed by its bounds.
    pub(super) fn show_managed_hover(&mut self, id: impl Into<ElementId>, content: HoverContent, options: HoverOptions, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.hovers.target = Some(HoverTarget { id: id.into(), content, options, bounds, mouse_x: None });
        self.hovers.dismissed = false;
        self.show_hover(false, window, cx);
    }

    fn show_hover(&mut self, fade_in: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.hovers.target.clone().filter(|_| !self.hovers.dismissed) else { return };
        self.hovers.pending = None;
        self.hovers.hiding = None;
        self.hovers.shown_count += 1;
        let text = self.hover_text(&target.content, window);
        // showInstantHover: no hover for empty content.
        if text.is_empty() {
            return;
        }
        self.hovers.shown = Some(ShownHover { target, text, fade_in, serial: self.hovers.shown_count });
        cx.notify();
    }

    /// hoverService.ts `_keyDown` and `_keyUp` of Alt: Alt locks the shown hover; releasing it hides the hover
    /// unless the pointer is on it or its target.
    pub(super) fn hover_modifiers_changed(&mut self, alt: bool, cx: &mut Context<Self>) {
        if self.hovers.shown.is_none() || alt == self.hovers.locked {
            return;
        }
        self.hovers.locked = alt;
        if !alt && !self.hovers.mouse_in_hover && self.hovers.target.is_none() {
            self.hovers.shown = None;
            self.hovers.last_hide = Some(cx.background_executor().now());
            cx.notify();
        }
    }

    /// `hideHover` on mouse down, and `hideOnKeyDown` for a key no keybinding takes.
    pub(super) fn dismiss_hover(&mut self, cx: &mut Context<Self>) {
        self.hovers.pending = None;
        self.hovers.dismissed = self.hovers.target.is_some();
        self.hovers.locked = false;
        self.hovers.mouse_in_hover = false;
        if self.hovers.shown.take().is_some() {
            self.hovers.last_hide = Some(cx.background_executor().now());
            cx.notify();
        }
    }

    fn hover_text(&self, content: &HoverContent, window: &Window) -> String {
        match content {
            HoverContent::Text(text) | HoverContent::Preview(text) | HoverContent::Markdown(text) => text.clone(),
            HoverContent::Action { title, command } => self.keybindings.resolver.append_keybinding(title, command, &self.key_context(window)),
            HoverContent::LanguageStatus => self.language_status_hover_text(),
        }
    }

    pub(super) fn hover_widget(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hover = self.hovers.shown.as_ref()?;
        let preview = matches!(hover.target.content, HoverContent::Preview(_));
        let markdown = matches!(hover.target.content, HoverContent::Markdown(_));
        let language_status = match &hover.target.content {
            HoverContent::LanguageStatus => Some(self.language_status_hover(window, cx)),
            _ => None,
        };
        let interactive_rows = language_status.is_some();
        let paragraphs: Vec<Vec<Node>> = match markdown {
            // A `---` paragraph is a rule, kept as a paragraph without nodes.
            true => hover.text.split("\n\n").map(|paragraph| if paragraph.trim() == "---" { Vec::new() } else { parse_linked_text(paragraph) }).collect(),
            false => vec![vec![Node::Text(hover.text.clone())]],
        };
        // shortcut: a markdown paragraph is one line of text; wrap it at 500px when one longer than that shows.
        let mut content_width = paragraphs.iter().map(|nodes| text_width(&linked_text_label(nodes), false, window)).fold(0., f32::max);
        if preview {
            content_width += text_width(" (", false, window) + text_width("preview", true, window) + text_width(" )", false, window) + FONT_SIZE;
        }
        let mut content_height = paragraphs_height(&paragraphs);
        // `.markdown-hover > .hover-contents`: max-width 500px, word-wrap.
        let wraps = !markdown && !preview && language_status.is_none();
        if wraps && content_width > TEXT_MAX_WIDTH {
            content_width = TEXT_MAX_WIDTH;
            content_height = wrapped_lines(&hover.text, window) as f32 * LINE_HEIGHT;
        }
        if let Some((content_size, _)) = &language_status {
            (content_width, content_height) = (content_size.width, content_size.height);
        }
        let size = Size { width: (content_width + 2. * PADDING_X).min(MAX_WIDTH - HOVER_BORDER_WIDTH) + HOVER_BORDER_WIDTH, height: content_height + 2. * PADDING_Y + HOVER_BORDER_WIDTH };
        let viewport = window.viewport_size();
        let target = hover.target.bounds;
        let target = Bounds { origin: point(f32::from(target.origin.x), f32::from(target.origin.y)), size: Size { width: f32::from(target.size.width), height: f32::from(target.size.height) } };
        let mouse_x = hover.target.mouse_x.filter(|_| hover.target.options.follows_mouse);
        let layout = layout_hover(Size { width: f32::from(viewport.width), height: f32::from(viewport.height) }, target, size, &hover.target.options, mouse_x);
        let radius = if layout.pointer.is_some() { POINTER_RADIUS } else { RADIUS };
        let foreground = self.color("editorHoverWidget.foreground");
        let border = self.color("editorHoverWidget.border");
        let high_contrast = self.theme.kind.is_high_contrast();
        let gear = self.app_root.join("node_modules/@vscode/codicons/src/icons/gear.svg");
        let panel = div()
            .id("workbench-hover")
            .absolute()
            .left(px(layout.origin.x))
            .top(px(layout.origin.y))
            .w(px(size.width))
            .h(px(size.height))
            .px(px(PADDING_X))
            .py(px(PADDING_Y))
            .border_1()
            .border_color(border)
            .rounded(px(radius))
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .text_color(foreground)
            .whitespace_nowrap()
            .when(high_contrast, |panel| panel.bg(self.color("editorHoverWidget.background")))
            .when(!high_contrast, |panel| {
                // --vscode-shadow-lg.
                panel.child(self.frosted_glass("editorHoverWidget.background", radius)).shadow(vec![BoxShadow {
                    color: rgba(0x00000024).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                    inset: false,
                }])
            })
            .when_some(language_status.map(|(_, rows)| rows), |panel, rows| panel.child(rows))
            .when(markdown, |panel| panel.child(self.markdown_paragraphs(paragraphs, cx)))
            .when(wraps, |panel| panel.child(div().whitespace_normal().child(hover.text.clone())))
            .when(preview, |panel| panel.child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .child(hover.text.clone())
                    .child(" (")
                    .child(div().italic().child("preview"))
                    .child(" ")
                    .child(
                        div()
                            .id("configure-preview-mode")
                            .cursor_pointer()
                            .child(gpui::svg().external_path(gear.to_string_lossy().into_owned()).size(px(FONT_SIZE)).text_color(self.color("textLink.foreground")))
                            .on_click(cx.listener(|workbench, _, window, cx| {
                                workbench.dismiss_hover(cx);
                                workbench.execute_command("workbench.action.openSettings", vec![serde_json::json!("workbench.editor.enablePreview")], window, cx);
                            })),
                    )
                    .child(")"),
            ))
            // hideOnHover is off for markdown with a link: the pointer may move onto the hover to click it. A locked
            // hover tracks the pointer too (`_lockMouseTracker`), to hide once Alt is up and the pointer leaves.
            .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                workbench.hovers.mouse_in_hover = *hovered;
                match hovered {
                    true if preview || markdown || interactive_rows || workbench.hovers.locked => workbench.hovers.hiding = None,
                    false if workbench.hovers.target.is_none() && !workbench.hovers.locked => workbench.hide_hover_later(cx),
                    _ => {}
                }
            }));
        let caret = layout.pointer.map(|offset| self.hover_pointer(layout.side, layout.origin, size, offset));
        let widget = div().absolute().inset_0().child(panel).children(caret);
        let element = match hover.fade_in {
            true => widget.with_animation(("hover-fade-in", hover.serial), Animation::new(Duration::from_millis(100)), |widget, delta| widget.opacity(delta)).into_any_element(),
            false => widget.into_any_element(),
        };
        Some(element)
    }

    fn markdown_paragraphs(&self, paragraphs: Vec<Vec<Node>>, cx: &mut Context<Self>) -> impl IntoElement {
        let link_color = self.color("textLink.foreground");
        let rule_color = self.theme.color("editorHoverWidget.border").map(|color| Hsla::from(color).opacity(0.5));
        let mut column = div().flex().flex_col();
        let mut after_rule = false;
        for (paragraph, nodes) in paragraphs.into_iter().enumerate() {
            let top = paragraph_top(paragraph, nodes.is_empty(), after_rule);
            after_rule = nodes.is_empty();
            if nodes.is_empty() {
                column = column.child(div().mt(px(top)).mx(px(-PADDING_X)).h(px(1.)).when_some(rule_color, |rule, color| rule.bg(color)));
                continue;
            }
            let mut row = div().flex().mt(px(top));
            for (index, node) in nodes.into_iter().enumerate() {
                row = match node {
                    Node::Text(text) => row.child(text),
                    Node::Link { label, href, .. } => row.child(
                        div().id(("hover-link", paragraph * 100 + index)).cursor_pointer().text_color(link_color).child(label).on_click(cx.listener(move |workbench, _, window, cx| {
                            workbench.dismiss_hover(cx);
                            workbench.open_notification_link(&href, window, cx);
                        })),
                    ),
                };
            }
            column = column.child(row);
        }
        column
    }

    /// `.workbench-hover-pointer:after`: a 6px square turned 45 degrees, centered on the hover's edge, showing the
    /// hover's background and its two outer borders.
    pub(super) fn hover_pointer(&self, side: HoverPosition, origin: Point<f32>, size: Size<f32>, offset: f32) -> impl IntoElement {
        let half = POINTER_SIZE * std::f32::consts::SQRT_2;
        let (center, outward) = match side {
            HoverPosition::Right => (point(origin.x, origin.y + offset), point(-1f32, 0.)),
            HoverPosition::Left => (point(origin.x + size.width, origin.y + offset), point(1., 0.)),
            HoverPosition::Below => (point(origin.x + offset, origin.y), point(0., -1.)),
            HoverPosition::Above => (point(origin.x + offset, origin.y + size.height), point(0., 1.)),
        };
        let along = point(outward.y.abs(), outward.x.abs());
        let at = move |a: f32, o: f32| point(px(center.x + along.x * a + outward.x * o), px(center.y + along.y * a + outward.y * o));
        let mut background = self.color("editorHoverWidget.background");
        if !self.theme.kind.is_high_contrast() {
            background.a *= 0.92;
        }
        let border = self.color("editorHoverWidget.border");
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                // Covers the hover's border between the caret's arms, which the square hides in the reference.
                let mut fill = PathBuilder::fill();
                fill.move_to(at(-half, 1.));
                fill.line_to(at(0., half));
                fill.line_to(at(half, 1.));
                fill.close();
                if let Ok(path) = fill.build() {
                    window.paint_path(path, background);
                }
                let mut arms = PathBuilder::stroke(px(1.));
                arms.move_to(at(-half + 0.5, 0.5));
                arms.line_to(at(0., half - 0.2));
                arms.line_to(at(half - 0.5, 0.5));
                if let Ok(path) = arms.build() {
                    window.paint_path(path, border);
                }
            },
        )
        .absolute()
        .inset_0()
    }
}

impl Hovers {
    fn is_target(&self, id: &ElementId) -> bool {
        self.target.as_ref().is_some_and(|target| &target.id == id)
    }
}

/// hover.css: a paragraph has an 8px margin between paragraphs; a rule (`hr`: 4px top, -4px bottom margin, a 1px
/// line) collapses with them into 8px above it and 4px below it.
fn paragraph_top(index: usize, rule: bool, after_rule: bool) -> f32 {
    match (index, rule, after_rule) {
        (0, _, _) => 0.,
        (_, false, true) => 4.,
        _ => PARAGRAPH_GAP,
    }
}

fn paragraphs_height(paragraphs: &[Vec<Node>]) -> f32 {
    let mut height = 0.;
    let mut after_rule = false;
    for (index, nodes) in paragraphs.iter().enumerate() {
        height += paragraph_top(index, nodes.is_empty(), after_rule) + if nodes.is_empty() { 1. } else { LINE_HEIGHT };
        after_rule = nodes.is_empty();
    }
    height
}

fn wrapped_lines(text: &str, window: &Window) -> usize {
    let run = TextRun { len: text.len(), font: window.text_style().font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    let lines = window.text_system().shape_text(text.to_string().into(), px(FONT_SIZE), &[run], Some(px(TEXT_MAX_WIDTH)), None);
    lines.map(|lines| lines.iter().map(|line| line.wrap_boundaries().len() + 1).sum()).unwrap_or(1)
}

fn linked_text_label(nodes: &[Node]) -> String {
    nodes.iter().map(|node| match node {
        Node::Text(text) => text.as_str(),
        Node::Link { label, .. } => label.as_str(),
    }).collect()
}

pub(super) fn text_width(text: &str, italic: bool, window: &Window) -> f32 {
    let mut font = window.text_style().font();
    if italic {
        font.style = gpui::FontStyle::Italic;
    }
    let run = TextRun { len: text.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(FONT_SIZE), &[run], None).width)
}

#[derive(Debug, PartialEq)]
pub struct HoverLayout {
    /// The hover's border box.
    pub origin: Point<f32>,
    /// Where the hover went after the fallbacks.
    pub side: HoverPosition,
    /// The pointer caret's center along the side facing the target, from the box's origin.
    pub pointer: Option<f32>,
}

/// hoverWidget.ts `layout`, then the context view's `layout2d` of the anchor it computes. `size` is the border box.
pub fn layout_hover(viewport: Size<f32>, target: Bounds<f32>, size: Size<f32>, options: &HoverOptions, mouse_x: Option<f32>) -> HoverLayout {
    let pointer = !options.follows_mouse;
    let pointer_offset = if pointer { POINTER_SIZE } else { 0. };
    // clientWidth and clientHeight are whole pixels.
    let client = Size { width: (size.width - HOVER_BORDER_WIDTH).round(), height: (size.height - HOVER_BORDER_WIDTH).round() };
    let hover_width = client.width + HOVER_BORDER_WIDTH;
    let (mut left, mut top) = (target.origin.x, target.origin.y);
    let (width, height) = (target.size.width, target.size.height);
    let mut side = options.position;

    // adjustHorizontalHoverPosition
    if mouse_x.is_none() {
        let room_right = viewport.width - (left + width);
        let needed = client.width + pointer_offset;
        if side == HoverPosition::Right && room_right < needed {
            side = if left >= needed { HoverPosition::Left } else { HoverPosition::Below };
        } else if side == HoverPosition::Left {
            if left < needed {
                side = if room_right >= needed { HoverPosition::Right } else { HoverPosition::Below };
            }
            if side == HoverPosition::Left && left - needed <= 0. {
                side = HoverPosition::Right;
            }
        }
    }

    // adjustVerticalHoverPosition
    let max_height = viewport.height * options.max_height_ratio;
    if side == HoverPosition::Above && top - client.height.min(max_height) - pointer_offset < 0. {
        side = HoverPosition::Below;
    } else if side == HoverPosition::Below && top + height + size.height.min(max_height) + pointer_offset > viewport.height {
        side = HoverPosition::Above;
    }

    if pointer {
        match side {
            HoverPosition::Right => left += POINTER_SIZE,
            HoverPosition::Left => left -= POINTER_SIZE,
            HoverPosition::Below => top += POINTER_SIZE,
            HoverPosition::Above => top -= POINTER_SIZE,
        }
    }
    let (right, bottom) = (left + width, top + height);
    let center = point(left + width / 2., top + height / 2.);

    // computeXCordinate
    let mut right_aligned = false;
    let mut x = match (mouse_x, side) {
        (Some(x), _) => x,
        (None, HoverPosition::Right) => right,
        (None, HoverPosition::Left) => left - hover_width,
        (None, _) => {
            let x = if pointer { center.x - client.width / 2. } else { left };
            right_aligned = x + hover_width >= viewport.width;
            if right_aligned { (viewport.width - hover_width - WINDOW_EDGE_MARGIN).max(0.) } else { x }
        }
    };
    if x < 0. {
        x = left + WINDOW_EDGE_MARGIN;
    }

    // computeYCordinate
    let mut y = match side {
        HoverPosition::Above => top,
        HoverPosition::Below => bottom - 2.,
        HoverPosition::Left | HoverPosition::Right => {
            let y = if pointer { center.y + client.height / 2. } else { bottom };
            y.min(viewport.height - WINDOW_EDGE_MARGIN).max(client.height + HOVER_BORDER_WIDTH + WINDOW_EDGE_MARGIN)
        }
    };
    if y > viewport.height {
        y = bottom;
    }

    // The context view anchors the hover at (x, y), below it only for BELOW.
    let position = if side == HoverPosition::Below { LayoutAnchorPosition::Before } else { LayoutAnchorPosition::After };
    let anchor = Bounds { origin: point(x, y), size: Size { width: 1., height: 2. } };
    let origin = layout2d(viewport, size, anchor, position);
    let origin = point(f32::from(origin.x), f32::from(origin.y));

    // setHoverPointerPosition
    let pointer = pointer.then(|| match side {
        HoverPosition::Left | HoverPosition::Right if client.height > height => center.y - (y - client.height),
        HoverPosition::Left | HoverPosition::Right => client.height / 2.,
        HoverPosition::Above | HoverPosition::Below => {
            let caret_left = x + client.width / 2. - POINTER_SIZE;
            if caret_left < left || caret_left > right { center.x - x } else { client.width / 2. }
        }
    });
    // hover.css `.right-aligned { left: 1px }` moves the hover, not its container with the pointer.
    let shift = if right_aligned { 1. } else { 0. };
    HoverLayout { origin: point(origin.x + shift, origin.y), side, pointer: pointer.map(|pointer| pointer - shift) }
}
