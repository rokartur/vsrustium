//! actionWidget.ts and the row renderers of actionList.ts: the code action menu over the window, its keyboard and
//! mouse focus, the action bar under the rows, and the block that closes it on a click elsewhere. The list logic is
//! `action_list.rs`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, BoxShadow, Context, FontWeight, MouseButton, ScrollHandle, TextRun, Window, div, point, prelude::*, px, rgba};
use serde_json::{Value, json};

use super::Workbench;
use super::code_actions::{CodeActionSet, RowLook};
use crate::action_list::{ActionList, Item, ItemKind, Options};
use crate::commands::Outcome;
use crate::editor::{Position, visible_column};

const UI_FONT: &str = ".SystemUIFont";
const FONT_SIZE: f32 = 13.;
/// cornerRadius-large and cornerRadius-medium.
const RADIUS: f32 = 8.;
const ROW_RADIUS: f32 = 6.;
/// `.action-widget` padding and border.
const PADDING: f32 = 4.;
/// `.monaco-list-row` padding `0 12px 0 6px` and the 6px gaps around the 12px icon.
const ROW_PADDING_LEFT: f32 = 6.;
const ROW_PADDING_RIGHT: f32 = 12.;
const ICON_SIZE: f32 = 12.;
const GAP: f32 = 6.;
/// `.action-widget-action-bar`: 4px margin, 1px border, 2px padding and a 22px line.
const ACTION_BAR_HEIGHT: f32 = 4. + 1. + 2. + 22.;
/// contextview.ts `getAnchorRect`: a point anchor is a 1px wide, 2px high box.
const ANCHOR_HEIGHT: f32 = 2.;

/// Where the widget opens: under a model position of the active editor, or at a window point.
#[derive(Clone, Copy)]
pub(super) enum Anchor {
    Position(Position),
    Point(gpui::Point<f32>),
}

#[derive(Default)]
pub(super) struct ActionWidget {
    shown: Option<Shown>,
    /// `_showDisabled`.
    show_disabled: bool,
    scroll: ScrollHandle,
    /// The editor's content left the last render laid out, for anchors at a model position.
    pub(super) content_left: Rc<Cell<f32>>,
}

struct Shown {
    list: ActionList<usize>,
    looks: Vec<RowLook>,
    anchor: Anchor,
    /// The action bar: documentation commands, then Show or Hide Disabled (no command).
    bar: Vec<(String, Option<Value>)>,
    set: Rc<CodeActionSet>,
    include_disabled: bool,
    from_lightbulb: bool,
    show_above: Option<bool>,
    /// The pointer moved the focus last (`mouse-navigation`).
    mouse_navigation: bool,
}

impl Workbench {
    pub(super) fn register_action_widget_commands(&mut self) {
        self.commands.register("hideCodeActionWidget", |workbench, _, window, cx| {
            workbench.hide_action_widget(cx);
            window.focus(&workbench.editor_focus, cx);
            Outcome::done()
        });
        self.commands.register("selectNextCodeAction", |workbench, _, _, cx| {
            workbench.move_action_widget_focus(ActionList::focus_next, cx);
            Outcome::done()
        });
        self.commands.register("selectPrevCodeAction", |workbench, _, _, cx| {
            workbench.move_action_widget_focus(ActionList::focus_previous, cx);
            Outcome::done()
        });
        // shortcut: no refactor preview panel, so previewing applies the action as accepting does.
        for id in ["acceptSelectedCodeAction", "previewSelectedCodeAction"] {
            self.commands.register(id, |workbench, _, window, cx| {
                workbench.accept_action_widget_row(None, window, cx);
                Outcome::done()
            });
        }
        let sections: [(&str, fn(&mut ActionList<usize>) -> bool); 3] = [
            ("collapseSectionCodeAction", ActionList::collapse_focused_section),
            ("expandSectionCodeAction", ActionList::expand_focused_section),
            ("toggleSectionCodeAction", ActionList::toggle_focused_section),
        ];
        for (id, change) in sections {
            self.commands.register(id, move |workbench, _, _, cx| {
                if let Some(shown) = workbench.action_widget.shown.as_mut() {
                    change(&mut shown.list);
                    cx.notify();
                }
                Outcome::done()
            });
        }
        self.commands.register("clearFilterCodeActionWidget", |workbench, _, _, cx| {
            if let Some(shown) = workbench.action_widget.shown.as_mut()
                && shown.list.clear_filter()
            {
                cx.notify();
            }
            Outcome::done()
        });
    }

    /// `codeActionMenuVisible` and `codeActionMenuFilterFocused`.
    pub(super) fn action_widget_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("codeActionMenuVisible".into(), json!(self.action_widget.shown.is_some()));
        context.insert("codeActionMenuFilterFocused".into(), json!(self.action_widget.shown.as_ref().is_some_and(|shown| shown.list.filter_focused)));
    }

    pub(super) fn action_widget_visible(&self) -> bool {
        self.action_widget.shown.is_some()
    }

    pub(super) fn action_widget_shows_disabled(&self) -> bool {
        self.action_widget.show_disabled
    }

    #[cfg(test)]
    pub(super) fn action_widget_labels(&self) -> Vec<String> {
        let shown = self.action_widget.shown.as_ref().expect("the widget shows");
        shown.list.all.iter().map(|item| item.label.clone().unwrap_or_default()).collect()
    }

    /// The focused row's code action, for its `quickfix-edit-highlight`.
    pub(super) fn action_widget_focused_action(&self) -> Option<&Value> {
        let shown = self.action_widget.shown.as_ref()?;
        let index = shown.list.focused_item()?.item?;
        shown.set.all.iter().chain(&shown.set.valid).nth(index).map(|item| &item.action)
    }

    /// `ActionWidgetService.show`: the rows with the first focusable one focused.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_action_widget(&mut self, items: Vec<Item<usize>>, looks: Vec<RowLook>, anchor: Anchor, bar: Vec<(String, Option<Value>)>, set: Rc<CodeActionSet>, include_disabled: bool, from_lightbulb: bool, cx: &mut Context<Self>) {
        self.hide_action_widget(cx);
        let mut list = ActionList::new(items, Options::default());
        list.focus();
        self.action_widget.shown = Some(Shown { list, looks, anchor, bar, set, include_disabled, from_lightbulb, show_above: None, mouse_navigation: false });
        self.action_widget.scroll = ScrollHandle::new();
        cx.notify();
    }

    /// `hide`: the rows go and the set is released unless the light bulb still shows it.
    pub(super) fn hide_action_widget(&mut self, cx: &mut Context<Self>) {
        if let Some(shown) = self.action_widget.shown.take() {
            self.release_code_action_set(shown.set);
            cx.notify();
        }
    }

    fn move_action_widget_focus(&mut self, change: fn(&mut ActionList<usize>), cx: &mut Context<Self>) {
        let Some(shown) = self.action_widget.shown.as_mut() else { return };
        change(&mut shown.list);
        shown.mouse_navigation = false;
        if let Some(row) = shown.list.focused {
            self.action_widget.scroll.scroll_to_item(row);
        }
        cx.notify();
    }

    /// `acceptSelected` or a click on `row`: an action row applies its action and the widget hides.
    fn accept_action_widget_row(&mut self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shown) = self.action_widget.shown.as_mut() else { return };
        let accepted = match row {
            Some(row) => shown.list.accept_row(row, false, false),
            None => shown.list.accept_selected(false, false),
        };
        let crate::action_list::Accepted::Select { item, .. } = accepted else { return cx.notify() };
        let set = shown.set.clone();
        self.hide_action_widget(cx);
        self.accept_code_action(item, set, window, cx);
    }

    /// An action bar entry: a documentation command runs, Show or Hide Disabled shows the list again.
    fn run_action_widget_bar(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shown) = self.action_widget.shown.as_ref() else { return };
        let (_, command) = &shown.bar[index];
        match command.clone() {
            Some(command) => {
                self.hide_action_widget(cx);
                window.focus(&self.editor_focus, cx);
                let args = command["arguments"].as_array().cloned().unwrap_or_default();
                drop(self.execute_command(command["id"].as_str().unwrap_or_default(), args, window, cx));
            }
            None => {
                let (set, anchor, include_disabled, from_lightbulb) = (shown.set.clone(), shown.anchor, shown.include_disabled, shown.from_lightbulb);
                self.action_widget.show_disabled = !self.action_widget.show_disabled;
                self.show_code_action_list(set, anchor, include_disabled, from_lightbulb, cx);
            }
        }
    }

    /// The widget at its anchor, below it when it fits in the window, else above; with the block that closes it on a
    /// press anywhere else.
    // shortcut: the menu opens without the frosted glass scale animation (GPUI has no element transforms).
    pub(super) fn action_widget(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let content_left = self.action_widget.content_left.get();
        let window_size = window.viewport_size().map(f32::from);
        let anchor = match self.action_widget.shown.as_ref()?.anchor {
            Anchor::Point(at) => at,
            Anchor::Position(position) => {
                let editor = self.editors.active()?;
                let viewport = self.editors.viewport;
                let text = editor.document().lines.get(position.line)?.clone();
                let bounds = self.text_area_bounds.get();
                let x = visible_column(&text, position.column, editor.document().indentation.tab_size) as f32 * viewport.char_width - editor.scroll.left + content_left;
                let y = editor.line_top(editor.view_line(position.line), viewport) - editor.scroll.top + viewport.line_height;
                point(bounds.origin.x + x, bounds.origin.y + y)
            }
        };
        let bar_width: f32 = {
            let shown = self.action_widget.shown.as_ref()?;
            match shown.bar.is_empty() {
                true => 0.,
                false => 26. + 8. + shown.bar.iter().map(|(label, _)| text_width(label, 12., FontWeight::NORMAL, window) + 16.).sum::<f32>(),
            }
        };
        let shown = self.action_widget.shown.as_mut()?;
        let chrome = 2. * PADDING + 2. + if shown.bar.is_empty() { 0. } else { ACTION_BAR_HEIGHT };
        let list_height = shown.list.compute_height(anchor.y, ANCHOR_HEIGHT, window_size.height, chrome, &mut shown.show_above);
        let list_width = shown.list.compute_width(bar_width, |item| row_width(item, &shown.looks, &shown.list, window));
        let size = (list_width + 2. * PADDING + 2., list_height + chrome);
        let shown = self.action_widget.shown.as_ref()?;
        // contextview.ts `layout`: below the anchor's 1x2 box when it fits, else above it, else against the far edge.
        let below = anchor.y + ANCHOR_HEIGHT;
        let top = if below + size.1 <= window_size.height { below } else if anchor.y - size.1 >= 0. { anchor.y - size.1 } else { (window_size.height - size.1).max(0.) };
        let left = if anchor.x + size.0 <= window_size.width { anchor.x } else { (anchor.x + 1. - size.0).max(0.) };
        let high_contrast = self.theme.kind.is_high_contrast();
        let mut rows = Vec::new();
        for row in 0..shown.list.visible.len() {
            rows.push(self.action_widget_row(shown, row, cx));
        }
        let list = div().id("action-widget-list").h(px(list_height)).w(px(list_width)).overflow_y_scroll().track_scroll(&self.action_widget.scroll).children(rows);
        let bar = (!shown.bar.is_empty()).then(|| {
            let link = self.color("textLink.activeForeground");
            div()
                .mt(px(4.))
                .border_t_1()
                .border_color(self.color("editorHoverWidget.border"))
                .pt(px(2.))
                .pl(px(26.))
                .pr(px(8.))
                .flex()
                .children(shown.bar.iter().enumerate().map(|(index, (label, _))| {
                    div()
                        .id(("action-widget-bar", index))
                        .mr(px(16.))
                        .text_size(px(12.))
                        .line_height(px(22.))
                        .text_color(link)
                        .cursor_pointer()
                        .child(label.clone())
                        .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                            cx.stop_propagation();
                            workbench.run_action_widget_bar(index, window, cx);
                        }))
                }))
        });
        let widget = div()
            .id("action-widget")
            .absolute()
            .left(px(left))
            .top(px(top))
            .p(px(PADDING))
            .border_1()
            .border_color(self.color("editorHoverWidget.border"))
            .rounded(px(RADIUS))
            .font_family(UI_FONT)
            .text_size(px(FONT_SIZE))
            .text_color(self.foreground_or("menu.foreground"))
            .occlude()
            .when(high_contrast, |widget| widget.bg(self.color("menu.background")))
            .when(!high_contrast, |widget| {
                // --vscode-shadow-lg blurs 12px; GPUI's blur_radius is the sigma, half the CSS blur.
                widget.child(self.frosted_glass("menu.background", RADIUS)).shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(6.), spread_radius: px(0.), inset: false }])
            })
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(list)
            .children(bar);
        let block = div().id("action-widget-block").absolute().inset_0().occlude().on_any_mouse_down(cx.listener(|workbench, _, window, cx| {
            workbench.hide_action_widget(cx);
            window.focus(&workbench.editor_focus, cx);
        }));
        Some(div().absolute().inset_0().child(block).child(widget).into_any_element())
    }

    /// `HeaderRenderer` and `ActionItemRenderer`: a header's group title, an action's icon, title and keybinding.
    fn action_widget_row(&self, shown: &Shown, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let index = shown.list.visible[row];
        let item = &shown.list.all[index];
        let look = &shown.looks[index];
        let height = ActionList::<usize>::item_height(item);
        let base = div().id(("action-widget-row", row)).h(px(height)).flex_none().flex().items_center().pl(px(ROW_PADDING_LEFT)).pr(px(ROW_PADDING_RIGHT)).whitespace_nowrap().overflow_hidden();
        if item.kind == ItemKind::Header {
            // `.group-header:not(:first-of-type)` has a 2px top margin; the list places rows absolutely, so it only moves
            // the header itself.
            let header = base.when(row > 0, |header| header.relative().top(px(2.))).text_color(self.color("descriptionForeground")).font_weight(FontWeight::SEMIBOLD).text_size(px(11.)).child(item.label.clone().unwrap_or_default());
            return header.into_any_element();
        }
        let focused = shown.list.focused == Some(row) && !item.disabled;
        let foreground = match (item.disabled, focused) {
            (true, _) => Some(self.color("disabledForeground")),
            (false, true) => self.theme.color("list.hoverForeground"),
            (false, false) => None,
        };
        let icon_color = foreground.or_else(|| look.color.map(|key| self.color(key))).unwrap_or_else(|| self.foreground_or("menu.foreground"));
        let selection_border = self.theme.color("menu.selectionBorder");
        let mut row_element = base
            .gap(px(GAP))
            .rounded(px(ROW_RADIUS))
            .when(!item.disabled, |row| row.cursor_pointer())
            .when_some(foreground, |row, color| row.text_color(color))
            .when(focused, |row| row.bg(self.color("list.hoverBackground")))
            .child(self.icon(crate::symbol_icons::svg_name(look.icon), ICON_SIZE, ICON_SIZE, icon_color))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(item.label.clone().unwrap_or_default()));
        if focused && let Some(border) = selection_border {
            row_element = row_element.border_1().when(shown.mouse_navigation, |row| row.border_dashed()).border_color(border);
        }
        if let Some(keybinding) = &look.keybinding {
            row_element = row_element.child(div().ml(px(FONT_SIZE / 2.)).text_size(px(12.)).text_color(self.color("descriptionForeground")).child(keybinding.clone()));
        }
        row_element
            .on_mouse_move(cx.listener(move |workbench, _, _, cx| {
                let Some(shown) = workbench.action_widget.shown.as_mut() else { return };
                if shown.list.focused != Some(row) && shown.list.visible_item(row).focusable() {
                    shown.list.focused = Some(row);
                    shown.mouse_navigation = true;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _, window, cx| {
                cx.stop_propagation();
                workbench.accept_action_widget_row(Some(row), window, cx);
            }))
            .into_any_element()
    }
}

/// A row's width with `width: auto`: its padding, the icon and gaps, the title and the keybinding.
fn row_width(item: &Item<usize>, looks: &[RowLook], list: &ActionList<usize>, window: &Window) -> f32 {
    let label = item.label.as_deref().unwrap_or_default();
    let padding = ROW_PADDING_LEFT + ROW_PADDING_RIGHT;
    if item.kind == ItemKind::Header {
        return padding + text_width(label, 11., FontWeight::SEMIBOLD, window);
    }
    let index = list.all.iter().position(|candidate| std::ptr::eq(candidate, item)).expect("rows come from the list");
    let keybinding = looks[index].keybinding.as_deref().map_or(0., |keybinding| GAP + FONT_SIZE / 2. + text_width(keybinding, 12., FontWeight::NORMAL, window));
    // The empty `.action-item-additional-badges` flex box still takes one more gap (measured 5px wider in 1.147).
    padding + ICON_SIZE + 2. * GAP + text_width(label, FONT_SIZE, FontWeight::NORMAL, window) + keybinding
}

fn text_width(text: &str, size: f32, weight: FontWeight, window: &Window) -> f32 {
    let font = gpui::Font { weight, ..gpui::font(UI_FONT) };
    let run = TextRun { len: text.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(text.to_string().into(), px(size), &[run], None).width)
}
