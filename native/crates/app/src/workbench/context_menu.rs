//! The custom context menu: contextMenuHandler.ts showing a base/browser/ui/menu/menu.ts `Menu` in a context view,
//! placed by base/common/layout.ts `layout2d` and drawn as menu.ts `getMenuWidgetCSS` and defaultMenuStyles draw it.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, BoxShadow, Context, FocusHandle, Keystroke, ModifiersChangedEvent, MouseButton, MouseUpEvent, Pixels, Point, Size, Subscription,
    Task, TextRun, Window, div, point, prelude::*, px, rgba,
};
use serde_json::Value;

use super::Workbench;
use crate::keybindings;
use crate::menus::{MenuAction, MenuId, menu_actions, mnemonic_menu_label};

#[cfg(test)]
mod tests;

const FONT_SIZE: f32 = 13.;
const ITEM_HEIGHT: f32 = 24.;
/// The separator's 5px margins and its 1px border.
const SEPARATOR_HEIGHT: f32 = 11.;
/// The label's and the keybinding's `padding: 0 2em`.
const LABEL_PADDING: f32 = 26.;
const ITEM_MARGIN: f32 = 4.;
/// The action bar's 4px top and bottom padding.
const BAR_PADDING: f32 = 4.;
const MIN_WIDTH: f32 = 160.;
/// cornerRadius.large and cornerRadius.medium under Modern UI (roundedCorners.css).
const RADIUS: f32 = 8.;
const ITEM_RADIUS: f32 = 6.;

pub struct ContextMenu {
    pub(super) actions: Vec<MenuAction>,
    /// Each command's keybinding label, looked up when the menu showed.
    keybindings: HashMap<String, String>,
    origin: Point<Pixels>,
    pub(super) focused: Option<usize>,
    /// `keyboard-navigation`: the last input was a key, so the focused item shows its selection border.
    keyboard_navigation: bool,
    /// `ModifierKeyEmitter.keyStatus.altKey`: items with an `alt` command show it instead.
    alt: bool,
    /// Item mouse ups run their action only 100ms after the menu showed (`runOnceToEnableMouseUp`).
    mouse_up_enabled: bool,
    /// `menuActionOptions.arg` and `getActionsContext`, passed to the command that runs.
    args: Vec<Value>,
    focus: FocusHandle,
    /// None when nothing had focus, as a title bar right-click in a window without editors.
    pub(super) focus_to_return: Option<FocusHandle>,
    /// Counts the menus shown; a new count restarts the open animation.
    shown: usize,
    _enable_mouse_up: Task<()>,
    _blur: Subscription,
    _window_deactivated: Subscription,
}

impl Workbench {
    /// `showContextMenu`: the menu's items whose `when` holds in the current context, anchored below `anchor`.
    /// Focus returns to the element focused before, when the menu hides.
    pub(super) fn show_context_menu(&mut self, menu: MenuId, context: &HashMap<String, Value>, anchor: Bounds<Pixels>, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let actions = menu_actions(&self.menus, menu, context);
        self.show_context_menu_actions(actions, context, anchor, args, window, cx);
    }

    pub(super) fn show_context_menu_actions(&mut self, actions: Vec<MenuAction>, context: &HashMap<String, Value>, anchor: Bounds<Pixels>, args: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        if actions.is_empty() {
            return;
        }
        let mut keybindings = HashMap::new();
        for action in &actions {
            let MenuAction::Command { id, alt, .. } = action else { continue };
            for id in std::iter::once(id).chain(alt.as_ref().map(|(id, _)| id)) {
                if let Some(item) = self.keybindings.resolver.lookup_primary_keybinding(id, context) {
                    keybindings.insert(id.clone(), keybindings::label(&item.keybinding));
                }
            }
        }
        let width = menu_width(&actions, &keybindings, window);
        let viewport = window.viewport_size();
        let origin = layout2d(Size { width: f32::from(viewport.width), height: f32::from(viewport.height) }, Size { width, height: menu_height(&actions) }, anchor_rect(anchor), LayoutAnchorPosition::Before);
        let focus_to_return = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let enable_mouse_up = cx.spawn_in(window, async |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = workbench.update(cx, |workbench, _| {
                if let Some(menu) = workbench.context_menu.as_mut() {
                    menu.mouse_up_enabled = true;
                }
            });
        });
        // `menu.onDidBlur` and the window's blur hide the menu.
        let blur = cx.on_blur(&focus, window, |workbench, window, cx| workbench.hide_context_menu(window, cx));
        let window_deactivated = cx.observe_window_activation(window, |workbench, window, cx| {
            if !window.is_window_active() {
                workbench.hide_context_menu(window, cx);
            }
        });
        self.context_menus_shown += 1;
        self.context_menu = Some(ContextMenu {
            actions,
            keybindings,
            origin,
            focused: None,
            keyboard_navigation: false,
            alt: window.modifiers().alt,
            mouse_up_enabled: false,
            args,
            focus,
            focus_to_return,
            shown: self.context_menus_shown,
            _enable_mouse_up: enable_mouse_up,
            _blur: blur,
            _window_deactivated: window_deactivated,
        });
        cx.notify();
    }

    /// `hideContextView`: focus goes back to where it was if the menu still has it.
    pub(super) fn hide_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else { return };
        if menu.focus.is_focused(window) {
            match &menu.focus_to_return {
                Some(focus) => window.focus(focus, cx),
                None => window.blur(cx),
            }
        }
        cx.notify();
    }

    /// The `ActionBar` and `Menu` key handling; returns whether the menu used the key.
    pub(super) fn context_menu_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.context_menu.as_mut() else { return false };
        menu.keyboard_navigation = true;
        cx.notify();
        if keystroke.modifiers.modified() {
            return false;
        }
        let last = menu.actions.len() - 1;
        match keystroke.key.as_str() {
            "up" => menu.focused = focus_previous(&menu.actions, menu.focused.unwrap_or(0)),
            "down" => menu.focused = focus_next(&menu.actions, menu.focused.unwrap_or(last)),
            "home" | "pageup" => menu.focused = focus_next(&menu.actions, last),
            "end" | "pagedown" => menu.focused = focus_previous(&menu.actions, 0),
            "escape" => self.hide_context_menu(window, cx),
            // `triggerKeys` on macOS: Enter and Space on key down run the focused item, without its alt command.
            "enter" | "space" => {
                if let Some(index) = menu.focused {
                    self.run_context_menu_item(index, false, window, cx);
                }
            }
            // Menus stop tab navigation.
            "tab" => {}
            _ => return false,
        }
        true
    }

    /// `ActionRunner.run`: a disabled item does nothing; else the menu hides and the command runs with its args.
    fn run_context_menu_item(&mut self, index: usize, alt: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.as_ref() else { return };
        let MenuAction::Command { id, enabled: true, alt: alt_command, .. } = &menu.actions[index] else { return };
        let id = match alt_command {
            Some((alt_id, _)) if alt => alt_id.clone(),
            _ => id.clone(),
        };
        let args = menu.args.clone();
        self.hide_context_menu(window, cx);
        let outcome = self.execute_command(&id, args, window, cx);
        super::keyboard::report_failure(id, outcome, cx);
    }

    /// The menu over a block that takes every mouse down outside it and hides the menu (`blockMouse`).
    pub(super) fn context_menu_widget(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let high_contrast = self.theme.kind.is_high_contrast();
        let border = self.theme.color("menu.border").or(self.theme.color("editorWidget.border"));
        let rows: Vec<_> = menu.actions.iter().enumerate().map(|(index, action)| self.context_menu_item(menu, index, action, cx)).collect();
        let panel = div()
            .id("context-menu")
            .track_focus(&menu.focus)
            .absolute()
            .left(menu.origin.x)
            .top(menu.origin.y)
            // `min-width: 160px` holds for the content box, inside the 1px border.
            .min_w(px(MIN_WIDTH + 2.))
            .py(px(BAR_PADDING))
            .rounded(px(RADIUS))
            .when_some(border, |panel, border| panel.border_1().border_color(border))
            .text_size(px(FONT_SIZE))
            .text_color(self.foreground_or("menu.foreground"))
            .occlude()
            // Modern UI frosted glass outside high contrast; high contrast menus have no shadow.
            .when(high_contrast, |panel| panel.bg(self.color("menu.background")))
            .when(!high_contrast, |panel| {
                // --vscode-shadow-lg.
                panel.child(self.frosted_glass("menu.background", RADIUS)).shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }])
            })
            .on_modifiers_changed(cx.listener(|workbench, event: &ModifiersChangedEvent, _, cx| {
                if let Some(menu) = workbench.context_menu.as_mut() {
                    menu.alt = event.modifiers.alt;
                    cx.notify();
                }
            }))
            .children(rows);
        // Frosted glass menus open with a 0.97 scale animation and no fade.
        // shortcut: GPUI has no element transforms, so frosted glass menus open without their scale animation.
        let panel = match high_contrast {
            true => panel.with_animation(("context-menu-open", menu.shown), Animation::new(Duration::from_millis(83)), |panel, delta| panel.opacity(delta)).into_any_element(),
            false => panel.into_any_element(),
        };
        let block = div()
            .id("context-view-block")
            .absolute()
            .inset_0()
            .occlude()
            // The pointer leaving the menu unfocuses the item (`MOUSE_OUT`); `on_hover` would also fire on key presses.
            .on_mouse_move(cx.listener(|workbench, _, _, cx| {
                if let Some(menu) = workbench.context_menu.as_mut().filter(|menu| menu.focused.is_some() && !menu.keyboard_navigation) {
                    menu.focused = None;
                    cx.notify();
                }
            }))
            .on_any_mouse_down(cx.listener(|workbench, _, window, cx| workbench.hide_context_menu(window, cx)));
        Some(div().absolute().inset_0().child(block).child(panel).into_any_element())
    }

    fn context_menu_item(&self, menu: &ContextMenu, index: usize, action: &MenuAction, cx: &mut Context<Self>) -> AnyElement {
        let MenuAction::Command { id, title, enabled, checked, alt } = action else {
            // `.action-label.separator`: the bar's full width, 5px margins and a 1px bottom border.
            return div().my(px(5.)).border_b_1().border_color(self.color("menu.separatorBackground")).into_any_element();
        };
        // `getActionForAltMode`: an enabled item shows its alt command while Alt is held.
        let (id, title) = match alt {
            Some((alt_id, alt_title)) if menu.alt && *enabled => (alt_id, alt_title),
            _ => (id, title),
        };
        let enabled = *enabled;
        let selected = enabled && menu.focused == Some(index);
        let selection_border = self.theme.color("menu.selectionBorder").filter(|_| selected);
        let high_contrast = self.theme.kind.is_high_contrast();
        // Only keyboard focus shows the selection border, except in high contrast, where the pointer's is dashed.
        let outline = selection_border.filter(|_| menu.keyboard_navigation || high_contrast).map(|border| {
            let dashed = high_contrast && !menu.keyboard_navigation;
            let color = match dashed {
                true => self.theme.color("contrastActiveBorder").unwrap_or(border),
                false => border,
            };
            div().absolute().inset_0().rounded(px(ITEM_RADIUS)).border_1().when(dashed, |outline| outline.border_dashed()).border_color(color)
        });
        let keybinding_opacity = match (enabled, selected) {
            (false, _) => 0.4,
            (true, true) => 1.,
            (true, false) => 0.7,
        };
        let run = |alt_held: fn(&MouseUpEvent) -> bool| {
            cx.listener(move |workbench: &mut Workbench, event: &MouseUpEvent, window, cx| {
                cx.stop_propagation();
                if workbench.context_menu.as_ref().is_some_and(|menu| menu.mouse_up_enabled) {
                    workbench.run_context_menu_item(index, alt_held(event), window, cx);
                }
            })
        };
        div()
            .id(index)
            .relative()
            .h(px(ITEM_HEIGHT))
            .mx(px(ITEM_MARGIN))
            .flex()
            .items_center()
            .rounded(px(ITEM_RADIUS))
            .when(enabled, |item| item.cursor_pointer())
            // High contrast focused items have no background, only the outline.
            .when(selected && !high_contrast, |item| item.bg(self.color("list.hoverBackground")).when_some(self.theme.color("list.hoverForeground"), |item, color| item.text_color(color)))
            .children(outline)
            // `.menu-item-check`: the `menu-selection` codicon centered in the label's left 2em padding.
            .when(*checked, |item| {
                let color = match selected {
                    true => self.theme.color("list.hoverForeground"),
                    false => None,
                };
                item.child(self.icon("check", LABEL_PADDING, FONT_SIZE, color.unwrap_or_else(|| self.foreground_or("menu.foreground"))).absolute().left_0().top_0().h_full())
            })
            .child(
                div()
                    .flex_1()
                    .px(px(LABEL_PADDING))
                    .line_height(px(FONT_SIZE))
                    .when(!enabled, |label| label.text_color(self.color("disabledForeground")))
                    .child(mnemonic_menu_label(title)),
            )
            .child(div().px(px(LABEL_PADDING)).line_height(px(FONT_SIZE)).opacity(keybinding_opacity).child(menu.keybindings.get(id).cloned().unwrap_or_default()))
            .on_mouse_move(cx.listener(move |workbench, _, _, cx| {
                if let Some(menu) = workbench.context_menu.as_mut().filter(|menu| menu.focused != Some(index) || menu.keyboard_navigation) {
                    menu.keyboard_navigation = false;
                    menu.focused = Some(index);
                    cx.notify();
                }
            }))
            // The click runs the item's alt command when Alt is held (`BaseActionViewItem.onClick`).
            .on_mouse_up(MouseButton::Left, run(|event| event.modifiers.alt))
            .on_mouse_up(MouseButton::Right, run(|event| event.modifiers.alt))
            .into_any_element()
    }
}

/// The widest item's label and keybinding, each with its 2em padding, inside the items' margins and the border.
/// Alt titles do not count: the menu grows to the right when Alt shows them, as the DOM menu does.
fn menu_width(actions: &[MenuAction], keybindings: &HashMap<String, String>, window: &Window) -> f32 {
    let mut content = MIN_WIDTH;
    for action in actions {
        let MenuAction::Command { id, title, .. } = action else { continue };
        let keybinding = keybindings.get(id).map_or(0., |label| text_width(label, window));
        content = content.max(text_width(&mnemonic_menu_label(title), window) + keybinding + 4. * LABEL_PADDING + 2. * ITEM_MARGIN);
    }
    content + 2.
}

fn text_width(text: &str, window: &Window) -> f32 {
    let run = TextRun { len: text.len(), font: window.text_style().font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(FONT_SIZE), &[run], None).width)
}

/// The border, the bar's padding, the items and the separators.
fn menu_height(actions: &[MenuAction]) -> f32 {
    let items: f32 = actions.iter().map(|action| if *action == MenuAction::Separator { SEPARATOR_HEIGHT } else { ITEM_HEIGHT }).sum();
    2. + 2. * BAR_PADDING + items
}

/// `getAnchorRect` of a mouse event: the pointer as a 2px block, so the menu does not open under it.
fn anchor_rect(anchor: Bounds<Pixels>) -> Bounds<f32> {
    let size = match anchor.size.width == px(0.) {
        true => Size { width: 2., height: 2. },
        false => Size { width: f32::from(anchor.size.width), height: f32::from(anchor.size.height) },
    };
    Bounds { origin: point(f32::from(anchor.origin.x), f32::from(anchor.origin.y)), size }
}

/// `focusNext` from `start`, looping and skipping separators and disabled items (`focusOnlyEnabledItems`).
fn focus_next(actions: &[MenuAction], start: usize) -> Option<usize> {
    let mut index = start;
    loop {
        index = (index + 1) % actions.len();
        if is_focusable(&actions[index]) {
            return Some(index);
        }
        if index == start {
            return None;
        }
    }
}

fn focus_previous(actions: &[MenuAction], start: usize) -> Option<usize> {
    let mut index = start;
    loop {
        index = (index + actions.len() - 1) % actions.len();
        if is_focusable(&actions[index]) {
            return Some(index);
        }
        if index == start {
            return None;
        }
    }
}

fn is_focusable(action: &MenuAction) -> bool {
    matches!(action, MenuAction::Command { enabled: true, .. } | MenuAction::Submenu { .. })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LayoutAnchorPosition {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LayoutAnchorMode {
    Avoid,
    Align,
}

struct LayoutAnchor {
    offset: f32,
    size: f32,
    mode: LayoutAnchorMode,
    position: LayoutAnchorPosition,
}

/// layout.ts `layout`: where a view of `view_size` goes along one axis next to the anchor, and whether it flipped.
fn layout(viewport_size: f32, view_size: f32, anchor: &LayoutAnchor) -> f32 {
    let (after_boundary, before_boundary) = match anchor.mode {
        LayoutAnchorMode::Align => (anchor.offset, anchor.offset + anchor.size),
        LayoutAnchorMode::Avoid => (anchor.offset + anchor.size, anchor.offset),
    };
    match anchor.position {
        LayoutAnchorPosition::Before if view_size <= viewport_size - after_boundary => after_boundary,
        LayoutAnchorPosition::Before if view_size <= before_boundary => before_boundary - view_size,
        LayoutAnchorPosition::Before => (viewport_size - view_size).max(0.),
        LayoutAnchorPosition::After if view_size <= before_boundary => before_boundary - view_size,
        LayoutAnchorPosition::After if view_size <= viewport_size - after_boundary && before_boundary < view_size / 2. => after_boundary,
        LayoutAnchorPosition::After => 0.,
    }
}

/// layout.ts `layout2d` aligned left on the vertical axis: below the anchor (`Before`) or above it (`After`).
pub(super) fn layout2d(viewport: Size<f32>, view: Size<f32>, anchor: Bounds<f32>, position: LayoutAnchorPosition) -> Point<Pixels> {
    let vertical = LayoutAnchor { offset: anchor.origin.y, size: anchor.size.height, mode: LayoutAnchorMode::Avoid, position };
    let top = layout(viewport.height, view.height, &vertical);
    // A view over the anchor's rows moves aside instead of aligning with it.
    let overlaps = top < vertical.offset + vertical.size && vertical.offset < top + view.height;
    let mode = if overlaps { LayoutAnchorMode::Avoid } else { LayoutAnchorMode::Align };
    let horizontal = LayoutAnchor { offset: anchor.origin.x, size: anchor.size.width, mode, position: LayoutAnchorPosition::Before };
    point(px(layout(viewport.width, view.width, &horizontal)), px(top))
}
