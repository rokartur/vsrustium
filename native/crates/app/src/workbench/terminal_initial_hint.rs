//! terminal.initialHint.contribution.ts with AI features hidden: at a new terminal's empty prompt, "Show
//! suggestions ⌃Space." and "Start typing to dismiss or don't show this again.", dismissed by any key or click.

use alacritty_terminal::grid::Dimensions;
use gpui::{AnyElement, Bounds, MouseButton, MouseDownEvent, SharedString, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::terminal_view::grid;
use crate::commands::Outcome;
use crate::menus::MenuAction;

const TRIGGER_SUGGEST: &str = "workbench.action.terminal.triggerSuggest";
const DISABLE_HINT: &str = "workench.action.disableTerminalInitialHint";
/// terminalInitialHint.css container queries: compact up to 500px, nothing up to 240px.
const COMPACT_WIDTH: f32 = 500.;
const HIDDEN_WIDTH: f32 = 240.;

impl Workbench {
    pub(super) fn register_terminal_initial_hint_actions(&mut self) {
        self.commands.register(DISABLE_HINT, |workbench, _, _, _| {
            workbench.update_setting("terminal.integrated.initialHint", json!(false));
            Outcome::done()
        });
    }

    /// `_createHint`: the hint widget at the cursor of a focused terminal with shell integration, no command run
    /// yet and an empty prompt; never for feature or extension terminals or with `sendKeybindingsToShell`.
    // shortcut: the hint shows while its terminal is focused, where upstream keeps it once shown; enough for a new terminal.
    pub(super) fn terminal_initial_hint(&self, id: u32, focused: bool, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !focused || self.terminal_view.initial_hint_dismissed.contains(&id) || self.setting("terminal.integrated.initialHint") != json!(true) || self.setting("terminal.integrated.sendKeybindingsToShell") == json!(true) {
            return None;
        }
        let (instance, pane, metrics) = (self.terminal_view.instance(id)?, self.terminal_view.pane_bounds.get(&id)?, self.terminal_view.metrics.as_ref()?);
        let launch = &instance.launch;
        if launch.extension_pty || launch.is_feature_terminal || launch.hide_from_user || instance.prompt_input.state() == crate::terminal::prompt_input::PromptInputState::Unknown {
            return None;
        }
        if !instance.commands.is_empty() || !instance.prompt_input.value().is_empty() || instance.term.grid().display_offset() != 0 {
            return None;
        }
        let cursor = instance.term.grid().cursor.point;
        let cols = instance.term.columns();
        if cursor.column.0 == 0 {
            return None;
        }
        // `_getHintContent` has nothing to hint about without the suggest keybinding.
        let suggest_keys = self.terminal_suggest_key_caps()?;
        let origin = grid::grid_origin(*pane, instance.term.screen_lines(), metrics) - pane.origin;
        let width = (cols - cursor.column.0) as f32 * metrics.cell_width;
        Some(
            div()
                .id(("terminal-initial-hint", id as usize))
                .absolute()
                .left(origin.x + px((cursor.column.0 + 1) as f32 * metrics.cell_width))
                .top(origin.y + px(cursor.line.0 as f32 * metrics.cell_height))
                .w(px(width))
                .pl(px(4.))
                .font_family(metrics.family.clone())
                .text_size(px(metrics.font_size))
                .line_height(px(metrics.cell_height))
                .text_color(self.color("terminal.initialHintForeground"))
                .when(width > HIDDEN_WIDTH, |hint| hint.child(self.initial_hint_content(suggest_keys, width <= COMPACT_WIDTH, cx)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    workbench.terminal_view.initial_hint_dismissed.insert(id);
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let action = MenuAction::Command { id: DISABLE_HINT.into(), title: "Disable Initial Hint".into(), enabled: true, checked: false, alt: None };
                    let context = workbench.key_context(window);
                    workbench.show_context_menu_actions(vec![action], &context, Bounds { origin: event.position, size: Default::default() }, Vec::new(), window, cx);
                }))
                .into_any_element(),
        )
    }

    /// The key caps of `triggerSuggest` in the current context, when terminal suggest is on.
    fn terminal_suggest_key_caps(&self) -> Option<Vec<String>> {
        if self.setting("terminal.integrated.suggest.enabled") != json!(true) {
            return None;
        }
        let item = self.keybindings.resolver.lookup_primary_keybinding(TRIGGER_SUGGEST, &self.context_keys)?;
        Some(item.keybinding.iter().flat_map(|chord| chord.key_caps()).collect())
    }

    /// `_getHintContent`: "Show suggestions {keys}." then the dismiss prose, or in a narrow terminal the
    /// suggestion link above "Don't show this again".
    fn initial_hint_content(&self, keys: Vec<String>, compact: bool, cx: &mut gpui::Context<Self>) -> AnyElement {
        let link_color = self.color("textLink.foreground");
        let link = |id: &'static str, label: &'static str, command: &'static str, args: Value| {
            div().id(id).italic().cursor_pointer().text_color(link_color).child(label).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                    workbench.execute_command(command, vec![args.clone()], window, cx);
                }),
            )
        };
        let suggest = div()
            .flex()
            .items_center()
            .child(link("terminal-initial-hint-suggest", "Show suggestions ", TRIGGER_SUGGEST, json!(null)))
            // The hint's `KeybindingLabel` gets no styles, so its keys draw without a background.
            .children(keys.iter().map(|key| self.key_cap(key, true)))
            .when(!compact, |line| line.child(link("terminal-initial-hint-suggest-after", ". ", TRIGGER_SUGGEST, json!(null))));
        let dismiss = match compact {
            true => div().flex().child(link("terminal-initial-hint-disable", "Don't show this again", DISABLE_HINT, json!(null))),
            false => div()
                .flex()
                .child(SharedString::from(" Start typing to dismiss or "))
                .child(link("terminal-initial-hint-disable", "don't show", DISABLE_HINT, json!(null)))
                .child(div().italic().child(" this again.")),
        };
        let content = if compact { div().flex().flex_col().child(suggest).child(dismiss) } else { div().flex().flex_wrap().child(suggest).child(dismiss) };
        content.into_any_element()
    }
}
