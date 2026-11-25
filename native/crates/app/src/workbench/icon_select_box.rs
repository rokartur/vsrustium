//! base/browser/ui/icons/iconSelectBox.ts `IconSelectBox` and workbench/services/userDataProfile/browser/iconSelectBox.ts
//! `WorkbenchIconSelectBox` with the Profiles editor's `ProfileIconRenderer`: the "Search icons" input over a grid of
//! userDataProfileIcons.ts `ICONS`, shown in a sticky hover below the profile's icon.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, Bounds, Context, Keystroke, Pixels, Size, Window, div, point, prelude::*, px};

use super::Workbench;
use super::hover::{HoverOptions, layout_hover};
use super::input_box_view::{InputId, InputLook, InputView};
use crate::commands::Outcome;
use crate::input_box::InputBox;

/// userDataProfileIcons.ts `ICONS`, as codicon ids.
pub const ICONS: [&str; 83] = [
    "vm", "server", "record-keys", "device-mobile", "watch", "vr", "piano", "ruby", "code", "coffee", "snake", "project", "window", "library", "extensions", "terminal",
    "terminal-debian", "terminal-linux", "terminal-ubuntu", "beaker", "package", "cloud", "book", "globe", "database", "notebook", "robot", "game", "chip", "music",
    "remote-explorer", "github", "azure", "vscode", "copilot", "gift", "send", "bookmark", "briefcase", "megaphone", "comment", "telescope", "credit-card", "map",
    "device-camera-video", "unmute", "law", "graph-line", "heart", "home", "inbox", "mortar-board", "rocket", "magnet", "lock", "milestone", "tag", "pulse",
    "radio-tower", "smiley", "zap", "squirrel", "symbol-color", "mail", "key", "pie-chart", "organization", "preview", "wand", "star-empty", "lightbulb",
    "symbol-ruler", "dashboard", "calendar", "shield", "verified", "debug", "flame", "compass", "paintcan", "archive", "mic", "jersey",
];

/// userDataProfileIcons.ts `DEFAULT_ICON` (`settings-view-bar-icon`, drawn as the settings gear).
pub const DEFAULT_ICON: &str = "settings-view-bar-icon";

/// `ProfileIconRenderer`'s `iconSelectBox.layout(new Dimension(486, 292))`.
const WIDTH: f32 = 486.;
const HEIGHT: f32 = 292.;
/// `iconContainerWidth` and `iconContainerHeight`.
const ICON_BOX: f32 = 36.;
/// `layout`: `floor((486 - 30) / 36)` icons per row, the 24px left over spread as a 2px right margin each.
const PER_ROW: usize = 12;
const ICON_MARGIN: f32 = 2.;

/// Which profile the picker sets the icon of: an existing one by id, or the new profile form.
#[derive(Clone, Debug, PartialEq)]
pub enum IconTarget {
    Profile(String),
    NewProfile,
}

/// The shown picker: its search, the icons matching it and the focused one.
pub struct IconSelectBox {
    pub target: IconTarget,
    pub search: InputBox,
    pub search_view: InputView,
    rendered: Vec<&'static str>,
    focused: Option<usize>,
    /// The icons container has the keyboard focus, not the search input.
    pub grid_focused: bool,
}

impl IconSelectBox {
    /// `clearInput` then `focus`: every icon, the first one focused, the input focused.
    pub fn new(target: IconTarget) -> Self {
        IconSelectBox { target, search: InputBox::new(String::new()), search_view: InputView::default(), rendered: ICONS.to_vec(), focused: Some(0), grid_focused: false }
    }

    /// `onDidChange` of the input: the icons whose id contains the value (`matchesContiguous`); the focused icon
    /// stays focused when it still shows, else the first one is.
    pub fn filter(&mut self) {
        let previous = self.focused.and_then(|index| self.rendered.get(index).copied());
        let query = self.search.value.to_lowercase();
        self.rendered = ICONS.iter().copied().filter(|icon| icon.contains(&query)).collect();
        let index = previous.and_then(|icon| self.rendered.iter().position(|rendered| *rendered == icon)).unwrap_or(0);
        self.focus_icon(index as isize);
    }

    /// `focusIcon`: clamped to the rendered icons, none when there are none.
    fn focus_icon(&mut self, index: isize) {
        self.focused = match self.rendered.len() {
            0 => None,
            len => Some(index.clamp(0, len as isize - 1) as usize),
        };
    }

    fn focused_index(&self) -> isize {
        self.focused.map_or(-1, |index| index as isize)
    }

    pub fn focus_next(&mut self) {
        self.focus_icon(self.focused_index() + 1);
    }

    pub fn focus_previous(&mut self) {
        self.focus_icon(self.focused_index() - 1);
    }

    /// `focusNextRow`: from the input to the icons, else one row down.
    pub fn focus_next_row(&mut self) {
        if !self.rendered.is_empty() && !self.grid_focused {
            self.grid_focused = true;
            return;
        }
        self.focus_icon(self.focused_index() + PER_ROW as isize);
    }

    /// `focusPreviousRow`: from the first row to the input, else one row up.
    pub fn focus_previous_row(&mut self) {
        if self.focused_index() < PER_ROW as isize {
            self.grid_focused = false;
            return;
        }
        self.focus_icon(self.focused_index() - PER_ROW as isize);
    }

    pub fn focus_first(&mut self) {
        self.focus_icon(0);
    }

    pub fn focus_last(&mut self) {
        self.focus_icon(self.rendered.len() as isize - 1);
    }

    /// What a key does in `onKeyDown`; `None` leaves the key to the input and the keybindings.
    fn key_action(&self, keystroke: &Keystroke) -> Option<KeyAction> {
        let key = keystroke.key.as_str();
        let modifiers = keystroke.modifiers;
        // ProfileIconRenderer's own listener on the box: Escape closes the hover.
        if key == "escape" && !modifiers.modified() {
            return Some(KeyAction::Close);
        }
        let input_focused = !self.grid_focused;
        if input_focused && matches!(key, "left" | "right" | "home" | "end") {
            return None;
        }
        if modifiers.alt || modifiers.control || modifiers.platform || modifiers.shift {
            return None;
        }
        if input_focused && !matches!(key, "down" | "up" | "enter") {
            return None;
        }
        Some(match key {
            "down" => KeyAction::NextRow,
            "up" => KeyAction::PreviousRow,
            "right" => KeyAction::Next,
            "left" => KeyAction::Previous,
            "home" => KeyAction::First,
            "end" => KeyAction::Last,
            "enter" | "space" => KeyAction::Select,
            _ => return None,
        })
    }
}

#[derive(Debug, PartialEq)]
enum KeyAction {
    Close,
    NextRow,
    PreviousRow,
    Next,
    Previous,
    First,
    Last,
    Select,
}

impl Workbench {
    /// iconSelectBox.ts's `iconSelectBox.*` commands, run on the shown picker.
    pub(super) fn register_icon_select_box_commands(&mut self) {
        let commands: [(&str, fn(&mut IconSelectBox)); 6] = [
            ("iconSelectBox.focusUp", IconSelectBox::focus_previous_row),
            ("iconSelectBox.focusDown", IconSelectBox::focus_next_row),
            ("iconSelectBox.focusNext", IconSelectBox::focus_next),
            ("iconSelectBox.focusPrevious", IconSelectBox::focus_previous),
            ("iconSelectBox.focusFirst", IconSelectBox::focus_first),
            ("iconSelectBox.focusLast", IconSelectBox::focus_last),
        ];
        for (id, focus) in commands {
            self.commands.register(id, move |workbench, _, window, cx| {
                if let Some(picker) = workbench.user_data_profiles.editor.icon_picker.as_mut() {
                    focus(picker);
                    workbench.focus_icon_select_box(window, cx);
                }
                Outcome::done()
            });
        }
        self.commands.register("iconSelectBox.selectFocused", |workbench, _, window, cx| {
            workbench.select_focused_icon(window, cx);
            Outcome::done()
        });
    }

    /// `showIconSelectBox`: the picker of `target` below the icon, its input focused.
    pub(super) fn show_icon_select_box(&mut self, target: IconTarget, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.user_data_profiles.editor;
        editor.icon_picker = Some(IconSelectBox::new(target));
        let focus = editor.icon_search_focus.get_or_insert_with(|| cx.focus_handle()).clone();
        editor.icon_grid_focus.get_or_insert_with(|| cx.focus_handle());
        window.focus(&focus, cx);
        self.blink_caret(InputId::IconSelectSearch, window, cx);
        cx.notify();
    }

    pub(super) fn hide_icon_select_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &mut self.user_data_profiles.editor;
        if editor.icon_picker.take().is_none() {
            return;
        }
        if let Some(focus) = editor.focus.clone() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn focus_icon_select_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = &self.user_data_profiles.editor;
        let Some(picker) = &editor.icon_picker else { return };
        let focus = if picker.grid_focused { editor.icon_grid_focus.clone() } else { editor.icon_search_focus.clone() };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn edit_icon_select_search(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        let Some(picker) = self.user_data_profiles.editor.icon_picker.as_mut() else { return };
        edit(&mut picker.search);
        picker.filter();
        cx.notify();
    }

    /// The box's key listener, ahead of the keybindings as its DOM listener runs before the window's.
    pub(super) fn icon_select_box_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let editor = &self.user_data_profiles.editor;
        let Some(picker) = &editor.icon_picker else { return false };
        let in_box = [&editor.icon_search_focus, &editor.icon_grid_focus].into_iter().flatten().any(|focus| focus.is_focused(window));
        if !in_box {
            return false;
        }
        let Some(action) = picker.key_action(keystroke) else { return false };
        let picker = self.user_data_profiles.editor.icon_picker.as_mut().expect("checked above");
        match action {
            KeyAction::Close => {
                self.hide_icon_select_box(window, cx);
                return true;
            }
            KeyAction::Select => {
                self.select_focused_icon(window, cx);
                return true;
            }
            KeyAction::NextRow => picker.focus_next_row(),
            KeyAction::PreviousRow => picker.focus_previous_row(),
            KeyAction::Next => picker.focus_next(),
            KeyAction::Previous => picker.focus_previous(),
            KeyAction::First => picker.focus_first(),
            KeyAction::Last => picker.focus_last(),
        }
        self.focus_icon_select_box(window, cx);
        true
    }

    fn select_focused_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.user_data_profiles.editor.icon_picker else { return };
        let Some(icon) = picker.focused.and_then(|index| picker.rendered.get(index).copied()) else { return };
        self.select_icon(icon, window, cx);
    }

    /// `onDidSelect`: the hover closes and the profile, or the new profile form, takes the icon.
    fn select_icon(&mut self, icon: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.user_data_profiles.editor.icon_picker else { return };
        let target = picker.target.clone();
        self.hide_icon_select_box(window, cx);
        self.set_profile_icon(&target, icon, cx);
    }

    /// The sticky hover with the picker, below `target` with its pointer, over the modal editor.
    pub(super) fn icon_select_box_widget(&self, target: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = &self.user_data_profiles.editor;
        let picker = editor.icon_picker.as_ref()?;
        let viewport = window.viewport_size();
        let target = Bounds { origin: point(f32::from(target.origin.x), f32::from(target.origin.y)), size: Size { width: f32::from(target.size.width), height: f32::from(target.size.height) } };
        // The 1px hover border around the 486x292 box.
        let size = Size { width: WIDTH + 2., height: HEIGHT + 2. };
        let layout = layout_hover(Size { width: f32::from(viewport.width), height: f32::from(viewport.height) }, target, size, &HoverOptions::sticky_below(), None);
        let high_contrast = self.theme.kind.is_high_contrast();
        let radius = 3.;
        let search_focused = editor.icon_search_focus.as_ref().is_some_and(|focus| focus.is_focused(window));
        let input_border = if search_focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or_default() };
        let search = div()
            .h(px(26.))
            .flex()
            .items_center()
            .px(px(6.))
            .border_1()
            .border_color(input_border)
            .bg(self.color("input.background"))
            .child(self.input_text(InputId::IconSelectSearch, InputLook { password: false, placeholder: Some("Search icons".into()), focused: search_focused }, cx));
        let panel = div()
            .id("icon-select-box")
            .absolute()
            .left(px(layout.origin.x))
            .top(px(layout.origin.y))
            .w(px(size.width))
            .h(px(size.height))
            .border_1()
            .border_color(self.color("editorHoverWidget.border"))
            .rounded(px(radius))
            .text_color(self.color("editorHoverWidget.foreground"))
            .when(high_contrast, |panel| panel.bg(self.color("editorHoverWidget.background")))
            .when(!high_contrast, |panel| panel.child(self.frosted_glass("editorHoverWidget.background", radius)))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // A sticky hover hides on a mouse down outside it.
            .on_mouse_down_out(cx.listener(|workbench, _, window, cx| workbench.hide_icon_select_box(window, cx)))
            .child(div().relative().mx(px(15.)).my(px(10.)).flex().flex_col().child(div().pb(px(10.)).child(search)).child(self.icon_grid(picker, window, cx)));
        let caret = layout.pointer.map(|offset| self.hover_pointer(layout.side, layout.origin, size, offset));
        Some(div().absolute().inset_0().child(panel).children(caret).into_any_element())
    }

    fn icon_grid(&self, picker: &IconSelectBox, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.user_data_profiles.editor;
        let grid_focus = editor.icon_grid_focus.clone();
        let grid_focused = grid_focus.as_ref().is_some_and(|focus| focus.is_focused(window));
        let foreground = self.color("editorHoverWidget.foreground");
        let focused_background = self.color("list.inactiveSelectionBackground");
        let focused_foreground = self.theme.color("list.inactiveSelectionForeground").unwrap_or(foreground);
        let hover_background = self.color("toolbar.hoverBackground");
        let mut grid = div().id("icon-select-icons").h(px(HEIGHT - 40.)).flex().flex_wrap().content_start();
        if let Some(focus) = &grid_focus {
            grid = grid.track_focus(focus);
        }
        if picker.rendered.is_empty() {
            return grid.child("No results");
        }
        for (index, &icon) in picker.rendered.iter().enumerate() {
            let focused = picker.focused == Some(index);
            let color = if focused { focused_foreground } else { foreground };
            let cell = self
                .icon(icon, ICON_BOX, 20., color)
                .id(("icon-select-icon", index))
                .mr(px(ICON_MARGIN))
                .rounded(px(5.))
                .cursor_pointer()
                .when(focused, |cell| cell.bg(focused_background))
                .when(focused && grid_focused, |cell| cell.border_1().border_color(self.color("focusBorder")))
                .when(!focused, |cell| cell.hover(|cell| cell.bg(hover_background)))
                .on_click(cx.listener(move |workbench, _, window, cx| workbench.select_icon(icon, window, cx)));
            grid = grid.child(cell);
        }
        grid
    }

    /// The Icon row of a profile that is not the default one: the icon that opens the picker, and its description.
    pub(super) fn profile_icon_row(&self, icon: Option<&str>, target: IconTarget, bounds: Rc<Cell<Bounds<Pixels>>>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let foreground = self.color("foreground");
        let button = self
            .icon(profile_icon_file(icon), 20., 16., foreground)
            .id("profile-icon")
            .mr(px(4.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .child(super::output_view::bounds_tracker(bounds))
            .child(self.hover_target("profile-icon-hover", super::hover::HoverContent::Text("Click to change icon".into()), HoverOptions::element()))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _, window, cx| workbench.show_icon_select_box(target.clone(), window, cx)));
        div().flex().items_center().line_height(px(22.)).child(button).child(div().mt(px(-1.)).opacity(0.9).child("Profile icon to be shown in the activity bar"))
    }
}

/// The codicon file of a profile icon id: `DEFAULT_ICON` and a missing icon draw the settings gear, `zap` shares
/// `symbol-event`'s glyph (codiconsLibrary.ts).
pub fn profile_icon_file(icon: Option<&str>) -> &str {
    match icon {
        None | Some(DEFAULT_ICON) => "settings-gear",
        Some("zap") => "symbol-event",
        Some(icon) => icon,
    }
}

#[cfg(test)]
mod tests;
