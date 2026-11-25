//! The custom context menu: contextMenuHandler.ts showing a base/browser/ui/menu/menu.ts `Menu` in a context view,
//! placed by base/common/layout.ts `layout2d` and drawn as menu.ts `getMenuWidgetCSS` and defaultMenuStyles draw it.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, BoxShadow, Context, FocusHandle, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton, MouseUpEvent, Pixels, Point, Size, Subscription,
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

/// The submenu indicator: a 16px chevron in `padding: 0 1.8em` of its 16px font, pulled 20px right.
const SUBMENU_INDICATOR_WIDTH: f32 = 2. * 1.8 * 16. + 16. - 20.;
/// `SubmenuMenuActionViewItem`'s `showScheduler` and `hideScheduler`.
const SUBMENU_SHOW_DELAY: Duration = Duration::from_millis(250);
const SUBMENU_HIDE_DELAY: Duration = Duration::from_millis(750);

/// One open menu: the root, then each open submenu, opened from item `parent` of the level before it.
pub(super) struct MenuLevel {
    pub(super) actions: Vec<MenuAction>,
    pub(super) origin: Point<Pixels>,
    width: f32,
    pub(super) focused: Option<usize>,
    parent: Option<usize>,
}

pub struct ContextMenu {
    /// The root menu and its open submenus; keys go to the last.
    pub(super) levels: Vec<MenuLevel>,
    /// Each command's keybinding label, looked up when the menu showed.
    keybindings: HashMap<String, String>,
    /// The pending submenu show or hide.
    submenu_timer: Option<Task<()>>,
    /// `keyboard-navigation`: the last input was a key, so the focused item shows its selection border.
    keyboard_navigation: bool,
    /// `ModifierKeyEmitter.keyStatus.altKey`: items with an `alt` command show it instead.
    alt: bool,
    /// Item mouse ups run their action only 100ms after the menu showed (`runOnceToEnableMouseUp`).
    mouse_up_enabled: bool,
    /// `menuActionOptions.arg` and `getActionsContext`, passed to the command that runs.
    pub(super) args: Vec<Value>,
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
        self.collect_menu_keybindings(&actions, context, &mut keybindings);
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
            levels: vec![MenuLevel { actions, origin, width, focused: None, parent: None }],
            keybindings,
            submenu_timer: None,
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

    fn collect_menu_keybindings(&self, actions: &[MenuAction], context: &HashMap<String, Value>, keybindings: &mut HashMap<String, String>) {
        for action in actions {
            match action {
                MenuAction::Command { id, alt, .. } => {
                    for id in std::iter::once(id).chain(alt.as_ref().map(|(id, _)| id)) {
                        if let Some(item) = self.keybindings.resolver.lookup_primary_keybinding(id, context) {
                            keybindings.insert(id.clone(), keybindings::label(&item.keybinding));
                        }
                    }
                }
                MenuAction::Submenu { actions, .. } => self.collect_menu_keybindings(actions, context, keybindings),
                MenuAction::Separator => {}
            }
        }
    }

    /// `SubmenuMenuActionViewItem.createSubmenu`: the submenu of item `index` of `level` opens beside it, placed by
    /// `calculateSubmenuMenuLayout`, replacing any submenu open from that level.
    fn open_context_submenu(&mut self, level: usize, index: usize, select_first: bool, window: &Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.as_mut() else { return };
        menu.submenu_timer = None;
        // `Menu.trigger`: the submenu's item keeps the focus in its menu.
        menu.levels[level].focused = Some(index);
        if menu.levels.get(level + 1).is_some_and(|open| open.parent == Some(index)) {
            menu.levels.truncate(level + 2);
            if select_first && menu.levels[level + 1].focused.is_none() {
                let submenu = &mut menu.levels[level + 1];
                submenu.focused = focus_next(&submenu.actions, submenu.actions.len() - 1);
            }
            cx.notify();
            return;
        }
        let parent = &menu.levels[level];
        let MenuAction::Submenu { actions, .. } = &parent.actions[index] else { return };
        let actions = actions.clone();
        let item_top: f32 = parent.actions[..index].iter().map(row_height).sum();
        // The entry box: the item's row across the menu, grown by the bar's padding above and below.
        let entry = Bounds {
            origin: point(f32::from(parent.origin.x) + 1., f32::from(parent.origin.y) + 1. + item_top),
            size: Size { width: parent.width - 2., height: ITEM_HEIGHT + 2. * BAR_PADDING },
        };
        let width = menu_width(&actions, &menu.keybindings, window);
        let viewport = window.viewport_size();
        let viewport = Size { width: f32::from(viewport.width), height: f32::from(viewport.height) };
        let origin = submenu_layout(viewport, Size { width, height: menu_height(&actions) }, entry);
        let focused = if select_first { focus_next(&actions, actions.len() - 1) } else { None };
        menu.levels.truncate(level + 1);
        menu.levels.push(MenuLevel { actions, origin, width, focused, parent: Some(index) });
        cx.notify();
    }

    /// The pointer reached item `index` of `level`, or left the menus (`None`): a submenu item opens its submenu
    /// after 250ms if the pointer stays, any other move closes the submenu of that level after 750ms unless the
    /// pointer went back to its item.
    fn context_menu_hover(&mut self, level: usize, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.as_mut() else { return };
        menu.keyboard_navigation = false;
        for (at, open) in menu.levels.iter_mut().enumerate() {
            if at >= level {
                open.focused = None;
            }
        }
        menu.levels[level].focused = index;
        cx.notify();
        let opens = index.filter(|index| matches!(menu.levels[level].actions[*index], MenuAction::Submenu { .. }));
        let delay = match (opens, menu.levels.len() > level + 1) {
            (Some(_), _) => SUBMENU_SHOW_DELAY,
            (None, true) => SUBMENU_HIDE_DELAY,
            (None, false) => return,
        };
        menu.submenu_timer = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let Some(menu) = workbench.context_menu.as_mut() else { return };
                let still = menu.levels.get(level).is_some_and(|open| open.focused == index) && !menu.keyboard_navigation;
                match opens {
                    Some(index) if still => workbench.open_context_submenu(level, index, false, window, cx),
                    Some(_) => {}
                    None if menu.levels.get(level + 1).is_some_and(|open| menu.levels[level].focused != open.parent) => {
                        menu.levels.truncate(level + 1);
                        cx.notify();
                    }
                    None => {}
                }
            });
        }));
    }

    /// A menu beside `anchor` (`AnchorAxisAlignment.HORIZONTAL`, `AnchorAlignment.RIGHT`): right of it, top aligned
    /// with it, or bottom aligned where it does not fit below, as the activity bar's global activity menus open.
    pub(super) fn show_context_menu_beside(&mut self, actions: Vec<MenuAction>, context: &HashMap<String, Value>, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.show_context_menu_actions(actions, context, anchor, Vec::new(), window, cx);
        let Some(menu) = self.context_menu.as_mut() else { return };
        let root = &mut menu.levels[0];
        let viewport = window.viewport_size();
        let viewport = Size { width: f32::from(viewport.width), height: f32::from(viewport.height) };
        root.origin = layout2d_beside(viewport, Size { width: root.width, height: menu_height(&root.actions) }, anchor_rect(anchor));
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
        // The last open submenu has the focus.
        let level = menu.levels.len() - 1;
        let open = &mut menu.levels[level];
        let trigger_key = matches!(keystroke.key.as_str(), "enter" | "space");
        // `isTriggerKeyEvent`: Alt+Enter and Alt+Space run the focused item's alt command when it shows one.
        if trigger_key && keystroke.modifiers == (Modifiers { alt: true, ..Default::default() }) {
            let Some(index) = open.focused else { return false };
            if !matches!(&open.actions[index], MenuAction::Command { enabled: true, alt: Some(_), .. }) {
                return false;
            }
            self.run_context_menu_item(level, index, true, window, cx);
            return true;
        }
        if keystroke.modifiers.modified() {
            return false;
        }
        let last = open.actions.len() - 1;
        let on_submenu = open.focused.is_some_and(|index| matches!(open.actions[index], MenuAction::Submenu { .. }));
        match keystroke.key.as_str() {
            "up" => open.focused = focus_previous(&open.actions, open.focused.unwrap_or(0)),
            "down" => open.focused = focus_next(&open.actions, open.focused.unwrap_or(last)),
            "home" | "pageup" => open.focused = focus_next(&open.actions, last),
            "end" | "pagedown" => open.focused = focus_previous(&open.actions, 0),
            // A submenu's Left and Escape close it, back to its item.
            "left" | "escape" if level > 0 => {
                menu.levels.pop();
                menu.submenu_timer = None;
            }
            "escape" => self.hide_context_menu(window, cx),
            // Right, Enter and Space on a submenu item open it with its first item focused.
            "right" | "enter" | "space" if on_submenu => {
                let index = open.focused.unwrap_or_default();
                self.open_context_submenu(level, index, true, window, cx);
            }
            // `triggerKeys` on macOS: Enter and Space on key down run the focused item.
            "enter" | "space" => {
                if let Some(index) = open.focused {
                    self.run_context_menu_item(level, index, false, window, cx);
                }
            }
            // Menus stop tab navigation.
            "tab" => {}
            _ => return false,
        }
        true
    }

    /// `ActionRunner.run`: a disabled item does nothing; else the menu hides and the command runs with its args.
    fn run_context_menu_item(&mut self, level: usize, index: usize, alt: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.as_ref() else { return };
        if matches!(menu.levels[level].actions[index], MenuAction::Submenu { .. }) {
            // `SubmenuMenuActionViewItem.onClick`: a click opens the submenu.
            return self.open_context_submenu(level, index, true, window, cx);
        }
        let MenuAction::Command { id, enabled: true, alt: alt_command, .. } = &menu.levels[level].actions[index] else { return };
        let id = match alt_command {
            Some((alt_id, _)) if alt => alt_id.clone(),
            _ => id.clone(),
        };
        let args = menu.args.clone();
        self.hide_context_menu(window, cx);
        let outcome = self.execute_command(&id, args, window, cx);
        super::keyboard::report_failure(outcome, cx);
    }

    /// The menu over a block that takes every mouse down outside it and hides the menu (`blockMouse`).
    pub(super) fn context_menu_widget(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let panels: Vec<_> = (0..menu.levels.len()).map(|level| self.context_menu_panel(menu, level, cx)).collect();
        let block = div()
            .id("context-view-block")
            .absolute()
            .inset_0()
            .occlude()
            // The pointer leaving the menus unfocuses their items (`MOUSE_OUT`); `on_hover` would also fire on key presses.
            .on_mouse_move(cx.listener(|workbench, _, window, cx| {
                if workbench.context_menu.as_ref().is_some_and(|menu| menu.levels[0].focused.is_some() && !menu.keyboard_navigation) {
                    workbench.context_menu_hover(0, None, window, cx);
                }
            }))
            .on_any_mouse_down(cx.listener(|workbench, _, window, cx| workbench.hide_context_menu(window, cx)));
        Some(div().absolute().inset_0().child(block).children(panels).into_any_element())
    }

    fn context_menu_panel(&self, menu: &ContextMenu, level: usize, cx: &mut Context<Self>) -> AnyElement {
        let open = &menu.levels[level];
        let high_contrast = self.theme.kind.is_high_contrast();
        let border = self.theme.color("menu.border").or(self.theme.color("editorWidget.border"));
        let rows: Vec<_> = open.actions.iter().enumerate().map(|(index, action)| self.context_menu_item(menu, level, index, action, cx)).collect();
        let panel = div()
            .id(("context-menu", level))
            .when(level == 0, |panel| panel.track_focus(&menu.focus))
            .absolute()
            .left(open.origin.x)
            .top(open.origin.y)
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
                // --vscode-shadow-lg blurs 12px; GPUI's blur_radius is the sigma, half the CSS blur.
                panel.child(self.frosted_glass("menu.background", RADIUS)).shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(6.), spread_radius: px(0.), inset: false }])
            })
            .on_modifiers_changed(cx.listener(|workbench, event: &ModifiersChangedEvent, _, cx| {
                if let Some(menu) = workbench.context_menu.as_mut() {
                    menu.alt = event.modifiers.alt;
                    cx.notify();
                }
            }))
            .children(rows);
        // Frosted glass menus open with a 0.97 scale animation and no fade; submenus have neither (`:not(.monaco-submenu)`).
        // shortcut: GPUI has no element transforms, so frosted glass menus open without their scale animation.
        match high_contrast && level == 0 {
            true => panel.with_animation(("context-menu-open", menu.shown), Animation::new(Duration::from_millis(83)), |panel, delta| panel.opacity(delta)).into_any_element(),
            false => panel.into_any_element(),
        }
    }

    fn context_menu_item(&self, menu: &ContextMenu, level: usize, index: usize, action: &MenuAction, cx: &mut Context<Self>) -> AnyElement {
        // A submenu item is always enabled: native menus do not observe enablement on submenus (`updateEnabled`).
        let (id, title, enabled, checked, submenu) = match action {
            // `getActionForAltMode`: an enabled item shows its alt command while Alt is held.
            MenuAction::Command { alt: Some((alt_id, alt_title)), enabled: true, checked, .. } if menu.alt => (Some(alt_id), alt_title, true, *checked, false),
            MenuAction::Command { id, title, enabled, checked, .. } => (Some(id), title, *enabled, *checked, false),
            MenuAction::Submenu { title, checked, .. } => (None, title, true, *checked, true),
            MenuAction::Separator => {
                // `.action-label.separator`: the bar's full width, 5px margins and a 1px bottom border.
                return div().my(px(5.)).border_b_1().border_color(self.color("menu.separatorBackground")).into_any_element();
            }
        };
        let selected = enabled && menu.levels[level].focused == Some(index);
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
        let foreground = match selected {
            true => self.theme.color("list.hoverForeground"),
            false => None,
        }
        .unwrap_or_else(|| self.foreground_or("menu.foreground"));
        let trailing = match submenu {
            // `.submenu-indicator`: the `menu-submenu` chevron at 0.7 opacity, 8.8px from the item's right edge.
            true => div().flex_1().flex().justify_end().pr(px(1.8 * 16. - 20.)).opacity(0.7).child(self.icon("chevron-right", 16., 16., foreground)).into_any_element(),
            false => div().px(px(LABEL_PADDING)).line_height(px(FONT_SIZE)).opacity(keybinding_opacity).child(id.and_then(|id| menu.keybindings.get(id).cloned()).unwrap_or_default()).into_any_element(),
        };
        let run = |alt_held: fn(&MouseUpEvent) -> bool| {
            cx.listener(move |workbench: &mut Workbench, event: &MouseUpEvent, window, cx| {
                cx.stop_propagation();
                if workbench.context_menu.as_ref().is_some_and(|menu| menu.mouse_up_enabled) {
                    workbench.run_context_menu_item(level, index, alt_held(event), window, cx);
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
            .when(checked, |item| item.child(self.icon("check", LABEL_PADDING, FONT_SIZE, foreground).absolute().left_0().top_0().h_full()))
            .child(
                div()
                    .flex_1()
                    .px(px(LABEL_PADDING))
                    .line_height(px(FONT_SIZE))
                    .when(!enabled, |label| label.text_color(self.color("disabledForeground")))
                    .child(mnemonic_menu_label(title)),
            )
            .child(trailing)
            .on_mouse_move(cx.listener(move |workbench, _, window, cx| {
                if workbench.context_menu.as_ref().is_some_and(|menu| menu.levels[level].focused != Some(index) || menu.keyboard_navigation) {
                    workbench.context_menu_hover(level, Some(index), window, cx);
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
        let trailing = match action {
            MenuAction::Command { id, .. } => keybindings.get(id).map_or(0., |label| text_width(label, window)) + 2. * LABEL_PADDING,
            MenuAction::Submenu { .. } => SUBMENU_INDICATOR_WIDTH,
            MenuAction::Separator => continue,
        };
        let (MenuAction::Command { title, .. } | MenuAction::Submenu { title, .. }) = action else { continue };
        content = content.max(text_width(&mnemonic_menu_label(title), window) + 2. * LABEL_PADDING + trailing + 2. * ITEM_MARGIN);
    }
    content + 2.
}

fn text_width(text: &str, window: &Window) -> f32 {
    let run = TextRun { len: text.len(), font: window.text_style().font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(FONT_SIZE), &[run], None).width)
}

/// The border, the bar's padding, the items and the separators.
fn menu_height(actions: &[MenuAction]) -> f32 {
    2. + 2. * BAR_PADDING + actions.iter().map(row_height).sum::<f32>()
}

fn row_height(action: &MenuAction) -> f32 {
    if *action == MenuAction::Separator { SEPARATOR_HEIGHT } else { ITEM_HEIGHT }
}

/// layout.ts `layout2d` with `AnchorAxisAlignment.HORIZONTAL` and `AnchorAlignment.RIGHT`: after the anchor on the
/// horizontal axis, then aligned with it below, unless the view overlaps it and has to move aside.
pub(super) fn layout2d_beside(viewport: Size<f32>, view: Size<f32>, anchor: Bounds<f32>) -> Point<Pixels> {
    let horizontal = LayoutAnchor { offset: anchor.origin.x, size: anchor.size.width, mode: LayoutAnchorMode::Avoid, position: LayoutAnchorPosition::After };
    let left = layout(viewport.width, view.width, &horizontal);
    let overlaps = left < horizontal.offset + horizontal.size && horizontal.offset < left + view.width;
    let mode = if overlaps { LayoutAnchorMode::Avoid } else { LayoutAnchorMode::Align };
    let vertical = LayoutAnchor { offset: anchor.origin.y, size: anchor.size.height, mode, position: LayoutAnchorPosition::Before };
    point(px(left), px(layout(viewport.height, view.height, &vertical)))
}

/// `SubmenuMenuActionViewItem.calculateSubmenuMenuLayout` expanding right and below: beside the entry, else on its
/// other side, else over the menu 10px in; then from the entry's top down, or up so its bottom meets the entry's.
fn submenu_layout(viewport: Size<f32>, submenu: Size<f32>, mut entry: Bounds<f32>) -> Point<Pixels> {
    let horizontal = LayoutAnchor { offset: entry.origin.x, size: entry.size.width, mode: LayoutAnchorMode::Avoid, position: LayoutAnchorPosition::Before };
    let mut left = layout(viewport.width, submenu.width, &horizontal);
    if left >= entry.origin.x && left < entry.origin.x + entry.size.width {
        if entry.origin.x + 10. + submenu.width <= viewport.width {
            left = entry.origin.x + 10.;
        }
        entry.origin.y += 10.;
        entry.size.height = 0.;
    }
    let vertical = LayoutAnchor { offset: entry.origin.y, size: 0., mode: LayoutAnchorMode::Avoid, position: LayoutAnchorPosition::Before };
    let mut top = layout(viewport.height, submenu.height, &vertical);
    if top + submenu.height == entry.origin.y && top + entry.size.height + submenu.height <= viewport.height {
        top += entry.size.height;
    }
    point(px(left), px(top))
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
