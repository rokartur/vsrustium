//! floatingMenu.ts `FloatingEditorToolbar` and floatingMenu.css: the primary actions of `MenuId.EditorContent` at
//! the editor's `BOTTOM_RIGHT_CORNER`, run with the model's URI. One action draws as `.single-button`, more as a
//! toolbar whose first action is the primary one.

use gpui::{AnyElement, BoxShadow, ClickEvent, Context, Div, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, point, prelude::FluentBuilder, px, rgba};

use super::Workbench;
use super::define_keybinding::DEFINE_KEYBINDING_COMMAND;
use super::hover::{HoverContent, HoverOptions};
use crate::editor::{HORIZONTAL_SCROLLBAR_HEIGHT, OpenEditor, VERTICAL_SCROLLBAR_WIDTH};
use crate::keybindings;
use crate::menus::{CommandIcon, MenuAction, MenuId, menu_actions};

/// `--vscode-cornerRadius-medium` and `-small`.
const RADIUS_MEDIUM: f32 = 6.;
const RADIUS_SMALL: f32 = 4.;

/// One toolbar action: command, label, enabled.
pub(super) struct FloatingAction {
    pub(super) command: String,
    title: String,
    enabled: bool,
}

impl Workbench {
    /// overlayWidgets.ts `BOTTOM_RIGHT_CORNER`: right of both scrollbars and the minimap, above twice the
    /// horizontal scrollbar; the widget is 28px high with one action, 26px with more.
    pub(super) fn floating_menu(&self, editor: &OpenEditor, height: f32, minimap_width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let actions = self.floating_menu_actions(editor, window);
        if actions.is_empty() {
            return None;
        }
        let single = actions.len() == 1;
        let widget_height = if single { 28. } else { 26. };
        let resource = editor.document.uri();
        let context = self.key_context(window);
        let mut widget = div()
            .id("floating-menu")
            .absolute()
            .right(px(2. * VERTICAL_SCROLLBAR_WIDTH + minimap_width))
            .top(px(height - widget_height - 2. * HORIZONTAL_SCROLLBAR_HEIGHT))
            .h(px(widget_height))
            .flex()
            .items_center()
            .gap(px(4.))
            .font_family(".SystemUIFont")
            .text_size(px(11.))
            .text_color(self.color("foreground"));
        let high_contrast = self.theme.kind.is_high_contrast();
        if !single || high_contrast {
            widget = widget.bg(self.color("editorWidget.background")).rounded(px(RADIUS_MEDIUM)).when_some(self.theme.color("contrastBorder"), |widget, border| widget.border_1().border_color(border));
        }
        if !single {
            // --vscode-shadow-lg.
            widget = widget.px(px(4.)).py(px(2.)).overflow_hidden().shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }]);
        }
        for (index, action) in actions.into_iter().enumerate() {
            let keybinding = self.keybindings.resolver.lookup_primary_keybinding(&action.command, &context).map(|item| keybindings::label(&item.keybinding));
            widget = widget.child(self.floating_menu_action(action, keybinding, index, single, high_contrast, resource.clone(), cx));
        }
        Some(widget.into_any_element())
    }

    /// Define Keybinding over the user keybindings.json (its `when` is the profile's keybindings resource), then the
    /// menu's commands; submenus and separators are left out of the primary group.
    pub(super) fn floating_menu_actions(&self, editor: &OpenEditor, window: &Window) -> Vec<FloatingAction> {
        let mut actions = Vec::new();
        if self.is_user_keybindings(editor) {
            actions.push(FloatingAction { command: DEFINE_KEYBINDING_COMMAND.into(), title: "Define Keybinding".into(), enabled: true });
        }
        for action in menu_actions(&self.menus, MenuId::EditorContent, &self.key_context(window)) {
            if let MenuAction::Command { id, title, enabled, .. } = action {
                actions.push(FloatingAction { command: id, title, enabled });
            }
        }
        actions
    }

    /// `MenuEntryActionViewItem` with `updateLabel`'s `title (keybinding)`; the first action is `.primary`.
    #[allow(clippy::too_many_arguments, reason = "the toolbar's per-item inputs")]
    fn floating_menu_action(&self, action: FloatingAction, keybinding: Option<String>, index: usize, single: bool, high_contrast: bool, resource: serde_json::Value, cx: &mut Context<Self>) -> impl IntoElement {
        let primary = index == 0;
        let codicon = match self.command_icons.get(&action.command) {
            Some(CommandIcon::Codicon(name)) => Some(name.clone()),
            _ => None,
        };
        let tooltip = match &keybinding {
            Some(keybinding) => format!("{} ({keybinding})", action.title),
            None => action.title.clone(),
        };
        let label: Div = match &codicon {
            Some(name) => {
                let size = if single { 28. } else { 22. };
                let color = if primary { self.color("button.foreground") } else { self.color("foreground") };
                div().size(px(size)).flex().items_center().justify_center().child(self.icon(name, 16., 16., color))
            }
            None => {
                let item = div().flex().items_center().child(tooltip.clone());
                if single { item.h(px(28.)).px(px(8.)) } else { item.px(px(6.)).py(px(4.)).line_height(px(14.)) }
            }
        };
        let hover_background = self.color("button.hoverBackground");
        let command = action.command.clone();
        label
            .id(("floating-menu-action", index))
            .rounded(px(if single { RADIUS_MEDIUM } else { RADIUS_SMALL }))
            .when(primary, |label| label.bg(self.color("button.background")).text_color(self.color("button.foreground")).hover(move |style| style.bg(hover_background)))
            .when(single && !high_contrast, |label| {
                label.shadow(vec![BoxShadow { color: self.color("widget.shadow").into(), offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
            })
            .when(!action.enabled, |label| label.opacity(0.4))
            .when(action.enabled, |label| {
                label.cursor_pointer().on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    drop(workbench.execute_command(&command, vec![resource.clone()], window, cx));
                }))
            })
            .child(self.hover_target(("floating-menu-hover", index), HoverContent::Text(tooltip), HoverOptions::mouse()))
    }
}
