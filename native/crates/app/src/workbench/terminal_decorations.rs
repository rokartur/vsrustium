//! decorationAddon.ts and decorationStyles.ts, the gutter part: a circle left of each command's prompt line,
//! filled in `terminalCommandDecoration.successBackground` for exit code 0, an error mark in `errorBackground`
//! otherwise, and an outline in `defaultBackground` while the command runs or the prompt waits.

use alacritty_terminal::grid::Dimensions;
use gpui::{AnyElement, Bounds, ClipboardItem, MouseButton, MouseDownEvent, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::terminal_view::grid;
use crate::commands::Outcome;
use crate::menus::MenuAction;

/// decorationStyles.ts `DecorationStyles`.
const DEFAULT_DIMENSION: f32 = 16.;
const MARGIN_LEFT: f32 = -17.;
/// terminal.ts `defaultTerminalFontSize` on macOS, which `updateLayout` scales by.
const DEFAULT_FONT_SIZE: f32 = 12.;

impl Workbench {
    /// The decoration menu's commands, each with the instance id and the command's index as arguments.
    pub(super) fn register_terminal_decoration_actions(&mut self) {
        self.commands.register("terminal.rerunCommand", |workbench, args, window, cx| {
            let Some((instance, command)) = workbench.decorated_command(&args) else { return Outcome::done() };
            workbench.run_terminal_command(instance, command.command, true, window, cx);
            Outcome::done()
        });
        self.commands.register("terminal.copyCommand", |workbench, args, _, cx| {
            if let Some((_, command)) = workbench.decorated_command(&args) {
                cx.write_to_clipboard(ClipboardItem::new_string(command.command));
            }
            Outcome::done()
        });
        self.commands.register("terminal.copyCommandAndOutput", |workbench, args, _, cx| {
            if let Some((_, command)) = workbench.decorated_command(&args) {
                cx.write_to_clipboard(ClipboardItem::new_string(format!("{}\n{}", command.command, command.output.trim_end())));
            }
            Outcome::done()
        });
        self.commands.register("terminal.copyOutput", |workbench, args, _, cx| {
            if let Some((_, command)) = workbench.decorated_command(&args) {
                cx.write_to_clipboard(ClipboardItem::new_string(command.output.trim_end().to_owned()));
            }
            Outcome::done()
        });
        // xtermTerminal.ts `getSelectionAsHtml(command)`: the rows after the prompt, the output as the plain text.
        self.commands.register("terminal.copyOutputAsHtml", |workbench, args, _, cx| {
            let Some((instance, command)) = workbench.decorated_command(&args) else { return Outcome::done() };
            let (Some(start), Some(end)) = (command.prompt_line.or(command.command_line), command.end_line) else { return Outcome::done() };
            let (colors, family, size) = workbench.terminal_html_style();
            let Some(terminal) = workbench.terminal_view.instance(instance) else { return Outcome::done() };
            let html = terminal.serialize_html((start + 1, 0), (end - 1, usize::MAX), &colors.palette, &family, size);
            cx.write_to_clipboard(ClipboardItem::new_string(command.output.trim_end().to_owned()));
            crate::clipboard::write_html(&html);
            Outcome::done()
        });
        self.commands.register("workbench.action.terminal.copySelectionAsHtml", |workbench, _, _, cx| {
            let (colors, family, size) = workbench.terminal_html_style();
            let Some(terminal) = workbench.terminal_view.active_instance_mut() else { return Outcome::done() };
            let (Some(text), Some(html)) = (terminal.selection_text(), terminal.selection_html(&colors, &family, size)) else {
                workbench.notify(crate::notifications::Notification::new(crate::notifications::Severity::Warning, "The terminal has no selection to copy"));
                return Outcome::done();
            };
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            crate::clipboard::write_html(&html);
            Outcome::done()
        });
        self.commands.register("terminal.learnShellIntegration", |workbench, _, window, cx| {
            workbench.execute_command("vscode.open", vec![json!("https://code.visualstudio.com/docs/terminal/shell-integration")], window, cx)
        });
    }

    /// The theme's ANSI colors and xterm's `fontFamily` and `fontSize` options, which the HTML copy carries.
    fn terminal_html_style(&self) -> (crate::terminal::colors::TerminalColors, String, f32) {
        let family = Some(self.setting("terminal.integrated.fontFamily")).filter(|family| family.as_str().is_some_and(|family| !family.is_empty())).unwrap_or_else(|| self.setting("editor.fontFamily"));
        let size = self.setting("terminal.integrated.fontSize").as_f64().unwrap_or(12.) as f32;
        (crate::terminal::colors::TerminalColors::new(&self.theme), family.as_str().unwrap_or_default().to_owned(), size)
    }

    fn decorated_command(&self, args: &[Value]) -> Option<(u32, crate::terminal::FinishedCommand)> {
        let instance = args.first()?.as_u64()? as u32;
        let index = args.get(1)?.as_u64()? as usize;
        Some((instance, self.terminal_view.instance(instance)?.commands.get(index)?.clone()))
    }

    /// `_showContextMenu`: what a finished command's decoration offers.
    fn show_decoration_menu(&mut self, instance: u32, index: usize, position: gpui::Point<gpui::Pixels>, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let Some(command) = self.terminal_view.instance(instance).and_then(|terminal| terminal.commands.get(index)) else { return };
        let action = |id: &str, title: &str| MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked: false, alt: None };
        let mut actions = Vec::new();
        if !command.command.is_empty() {
            actions.extend([action("terminal.rerunCommand", "Rerun Command"), MenuAction::Separator, action("terminal.copyCommand", "Copy Command")]);
        }
        if !command.output.is_empty() {
            actions.extend([action("terminal.copyCommandAndOutput", "Copy Command and Output"), action("terminal.copyOutput", "Copy Output"), action("terminal.copyOutputAsHtml", "Copy Output as HTML")]);
        }
        if !actions.is_empty() {
            actions.push(MenuAction::Separator);
        }
        actions.extend([action("workbench.action.terminal.runRecentCommand", "Run Recent Command"), action("workbench.action.terminal.goToRecentDirectory", "Go To Recent Directory"), MenuAction::Separator, action("terminal.learnShellIntegration", "Learn About Shell Integration")]);
        let context = self.key_context(window);
        let anchor = Bounds { origin: position, size: Default::default() };
        self.show_context_menu_actions(actions, &context, anchor, vec![json!(instance), json!(index)], window, cx);
    }

    /// markNavigationAddon.ts `showCommandGuide`: a 3px bar (`.terminal-command-guide`, 1.5px borders 20px left of the
    /// text, moved 3px right) beside the rows of the command under the pointer.
    pub(super) fn terminal_command_guide(&self, id: u32) -> Option<AnyElement> {
        let (guide_id, rows) = self.terminal_view.command_guide.clone()?;
        let (instance, pane, metrics) = (self.terminal_view.instance(id)?, self.terminal_view.pane_bounds.get(&id)?, self.terminal_view.metrics.as_ref()?);
        if guide_id != id {
            return None;
        }
        let screen_rows = instance.term.screen_lines() as i64;
        let viewport_y = instance.viewport_y();
        let first = (rows.start - viewport_y).max(0);
        let last = (rows.end - viewport_y).min(screen_rows);
        if first >= last {
            return None;
        }
        let origin = grid::grid_origin(*pane, screen_rows as usize, metrics) - pane.origin;
        let guide = gpui::div()
            .absolute()
            .left(origin.x - px(20.) + px(3.))
            .top(origin.y + px(first as f32 * metrics.cell_height))
            .w(px(3.))
            .h(px((last - first) as f32 * metrics.cell_height))
            .bg(self.color("terminalCommandGuide.foreground"))
            .when(rows.start - viewport_y == first, |guide| guide.rounded_t(px(1.)))
            .when(rows.end - viewport_y == last, |guide| guide.rounded_b(px(1.)));
        Some(guide.into_any_element())
    }

    /// The decorations of the rows on screen, where the grid painted last (`terminal.integrated.shellIntegration.decorationsEnabled`).
    pub(super) fn terminal_decorations(&self, id: u32, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let enabled = self.setting("terminal.integrated.shellIntegration.decorationsEnabled");
        if !matches!(enabled.as_str(), Some("both" | "gutter")) {
            return Vec::new();
        }
        let (Some(instance), Some(pane), Some(metrics)) = (self.terminal_view.instance(id), self.terminal_view.pane_bounds.get(&id), self.terminal_view.metrics.as_ref()) else { return Vec::new() };
        let rows = instance.term.screen_lines() as i64;
        let origin = grid::grid_origin(*pane, rows as usize, metrics) - pane.origin;
        let viewport_y = instance.term.history_size() as i64 - instance.term.grid().display_offset() as i64;
        // `updateLayout`: smaller than the default font scales the decoration down, never up.
        let font_size = self.setting("terminal.integrated.fontSize").as_f64().unwrap_or(12.) as f32;
        let line_height = self.setting("terminal.integrated.lineHeight").as_f64().unwrap_or(1.) as f32;
        let scalar = (font_size / DEFAULT_FONT_SIZE).min(1.);
        let mut decorations = Vec::new();
        for (command, line, exit_code) in instance.decorated_commands() {
            let row = line - viewport_y;
            if !(0..rows).contains(&row) {
                continue;
            }
            // `getTerminalCommandDecorationState`: no exit code yet is the running (default) state.
            let (icon, color) = match exit_code {
                Some(0) => ("circle-filled", "terminalCommandDecoration.successBackground"),
                Some(_) => ("error-small", "terminalCommandDecoration.errorBackground"),
                None => ("circle", "terminalCommandDecoration.defaultBackground"),
            };
            let finished = exit_code.is_some().then_some(command).flatten();
            let size = scalar * DEFAULT_DIMENSION;
            let element = self
                .icon(icon, size, size, self.color(color))
                .id(("terminal-decoration", line as usize))
                .absolute()
                .left(origin.x + px(scalar * MARGIN_LEFT))
                .top(origin.y + px(row as f32 * metrics.cell_height))
                .h(px(size * line_height))
                .when_some(finished, |decoration, index| {
                    decoration
                        .cursor_pointer()
                        .rounded(px(5.))
                        .hover(|decoration| decoration.bg(self.color("toolbar.hoverBackground")))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.show_decoration_menu(id, index, event.position, window, cx);
                        }))
                });
            decorations.push(element.into_any_element());
        }
        decorations
    }
}
