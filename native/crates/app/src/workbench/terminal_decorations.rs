//! decorationAddon.ts and decorationStyles.ts, the gutter part: a circle left of each command's prompt line,
//! filled in `terminalCommandDecoration.successBackground` for exit code 0, an error mark in `errorBackground`
//! otherwise, and an outline in `defaultBackground` while the command runs or the prompt waits.

use alacritty_terminal::grid::Dimensions;
use gpui::{AnyElement, Bounds, ClipboardItem, MouseButton, MouseDownEvent, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::terminal_view::grid;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::menus::MenuAction;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};
use super::terminal_view::TerminalInput;

/// decorationStyles.ts `getTerminalDecorationHoverContent(command, undefined, true)` of a finished command.
fn decoration_hover(command: &crate::terminal::FinishedCommand, now: u64) -> String {
    let executed = crate::timeline::from_now_ago(command.timestamp as i64, now as i64);
    let duration = duration_string(command.duration);
    let status = match command.exit_code {
        Some(0) | None => format!("Command executed {executed} and took {duration}"),
        Some(-1) => format!("Command executed {executed}, took {duration} and failed"),
        Some(code) => format!("Command executed {executed}, took {duration} and failed (Exit Code {code})"),
    };
    format!("Show Command Actions\n\n---\n\n{status}")
}

/// date.ts `getDurationString(ms)`.
fn duration_string(ms: u64) -> String {
    let seconds = ms as f64 / 1000.;
    match seconds {
        s if s < 1. => format!("{ms}ms"),
        s if s < 60. => format!("{s}s"),
        s if s < 3600. => format!("{} mins", (s / 60.).round()),
        s if s < 86400. => format!("{} hrs", (s / 3600.).round()),
        s => format!("{} days", (s / 86400.).round()),
    }
}

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
            if command.is_trusted {
                workbench.run_terminal_command(instance, command.command, true, window, cx);
                return Outcome::done();
            }
            let reply = workbench.prompt_notification(crate::notifications::Severity::Info, format!("Do you want to run the command: {}", command.command), &["Yes", "No"]);
            cx.spawn_in(window, async move |workbench, cx| {
                if reply.await == Ok(Ok(json!(0))) {
                    let _ = workbench.update_in(cx, |workbench, window, cx| workbench.run_terminal_command(instance, command.command, true, window, cx));
                }
            })
            .detach();
            Outcome::done()
        });
        self.commands.register("terminal.toggleVisibility", |workbench, _, window, cx| {
            workbench.show_decoration_visibility_pick(window, cx);
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
        // terminalConfigurationService.ts `getFont`: the monospace and (macOS) AppleBraille fallbacks.
        let family = format!("{}, monospace, AppleBraille", family.as_str().unwrap_or_default());
        (crate::terminal::colors::TerminalColors::new(&self.theme), family, size)
    }

    fn decorated_command(&self, args: &[Value]) -> Option<(u32, crate::terminal::FinishedCommand)> {
        let instance = args.first()?.as_u64()? as u32;
        let index = args.get(1)?.as_u64()? as usize;
        Some((instance, self.terminal_view.instance(instance)?.commands.get(index)?.clone()))
    }

    /// `_showContextMenu`: what a finished command's decoration offers.
    /// `_createContextMenu`: a click shows the command's actions, a right click puts Toggle Visibility above them.
    fn show_decoration_menu(&mut self, instance: u32, index: usize, context_menu: bool, position: gpui::Point<gpui::Pixels>, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let Some(command) = self.terminal_view.instance(instance).and_then(|terminal| terminal.commands.get(index)) else { return };
        let action = |id: &str, title: &str| MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked: false, alt: None };
        let has_command = !command.command.is_empty();
        let has_output = !command.output.is_empty();
        let mut actions = Vec::new();
        if context_menu {
            actions.push(action("terminal.toggleVisibility", "Toggle Visibility"));
        }
        let fixes = self.terminal_quick_fix_menu_actions(instance, index);
        if !fixes.is_empty() {
            actions.extend(fixes);
            actions.push(MenuAction::Separator);
        }
        if has_command {
            actions.extend([action("terminal.rerunCommand", "Rerun Command"), MenuAction::Separator, action("terminal.copyCommand", "Copy Command")]);
        }
        if has_output {
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

    /// markNavigationAddon.ts `registerTemporaryDecoration`: the rows a navigation revealed, 20px left of the text
    /// (`.terminal-scroll-highlight`) with a 5px `focusBorder` bar, outlined for the first 350ms.
    pub(super) fn terminal_navigation_highlight(&self, id: u32) -> Option<AnyElement> {
        let (instance, pane, metrics) = (self.terminal_view.instance(id)?, self.terminal_view.pane_bounds.get(&id)?, self.terminal_view.metrics.as_ref()?);
        let highlight = instance.navigation_highlight.as_ref()?;
        let screen_rows = instance.term.screen_lines() as i64;
        let viewport_y = instance.viewport_y();
        let first = (highlight.rows.start() - viewport_y).max(0);
        let last = (highlight.rows.end() - viewport_y + 1).min(screen_rows);
        if first >= last {
            return None;
        }
        let origin = grid::grid_origin(*pane, screen_rows as usize, metrics) - pane.origin;
        let color = self.color("focusBorder");
        let outlined = std::time::Instant::now() < highlight.outline_until;
        let element = gpui::div()
            .absolute()
            .left(origin.x - px(20.))
            .top(origin.y + px(first as f32 * metrics.cell_height))
            .w(px(5. + instance.term.columns() as f32 * metrics.cell_width + if outlined { 1. } else { 0. }))
            .h(px((last - first) as f32 * metrics.cell_height))
            .border_color(color)
            .border_l(px(5.))
            .when(outlined, |element| {
                element
                    .border_r(px(1.))
                    .when(*highlight.rows.start() - viewport_y == first, |element| element.border_t(px(1.)))
                    .when(*highlight.rows.end() - viewport_y + 1 == last, |element| element.border_b(px(1.)))
            });
        Some(element.into_any_element())
    }

    /// Draws the navigation highlight again once its outline is gone.
    pub(super) fn end_navigation_outline_later(&self, cx: &mut gpui::Context<Self>) {
        cx.spawn(async |workbench, cx| {
            cx.background_executor().timer(crate::terminal::marks::OUTLINE_DURATION).await;
            let _ = workbench.update(cx, |_, cx| cx.notify());
        })
        .detach();
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
                    let hover = decoration_hover(&instance.commands[index], crate::terminal::now_ms());
                    decoration
                        .child(self.hover_target(("terminal-decoration-hover", line as usize), HoverContent::Markdown(hover), HoverOptions::element()))
                        .cursor_pointer()
                        .rounded(px(5.))
                        .hover(|decoration| decoration.bg(self.color("toolbar.hoverBackground")))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.show_decoration_menu(id, index, false, event.position, window, cx);
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            workbench.show_decoration_menu(id, index, true, event.position, window, cx);
                        }))
                });
            decorations.push(element.into_any_element());
        }
        decorations.extend(self.terminal_light_bulb(id, origin, viewport_y, rows, scalar, metrics.cell_height, cx));
        decorations
    }

    /// `_showToggleVisibilityQuickPick`: the gutter and overview ruler items, each change written to
    /// `terminal.integrated.shellIntegration.decorationsEnabled`.
    fn show_decoration_visibility_pick(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let value = self.setting("terminal.integrated.shellIntegration.decorationsEnabled");
        let value = value.as_str().unwrap_or("both");
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.hide_input = true;
        input.can_select_many = true;
        input.title = Some("Toggle visibility".into());
        let labels = ["Gutter command decorations", "Overview ruler command decorations"];
        input.set_items(labels.iter().enumerate().map(|(handle, label)| Entry::Item(Item { handle: handle as i64, label: (*label).into(), ..Item::default() })).collect());
        let mut selected = Vec::new();
        if value != "never" {
            selected.extend((value != "overviewRuler").then_some(0));
            selected.extend((value != "gutter").then_some(1));
        }
        input.set_selected_items(&selected);
        let id = self.quick_inputs.add(input, Owner::Terminal(TerminalInput::DecorationVisibility));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub(super) fn decoration_visibility_event(&mut self, event: Event) {
        let Event::ChangeSelection(handles) = event else { return };
        let value = match (handles.contains(&0), handles.contains(&1)) {
            (true, true) => "both",
            (true, false) => "gutter",
            (false, true) => "overviewRuler",
            (false, false) => "never",
        };
        self.update_setting("terminal.integrated.shellIntegration.decorationsEnabled", json!(value));
    }

    /// decorationAddon.ts `registerCommandDecoration` with `overviewRulerOptions`: a mark per command, right when it failed.
    pub(super) fn terminal_overview_ruler(&self, id: u32) -> Option<grid::OverviewRuler> {
        let enabled = self.setting("terminal.integrated.shellIntegration.decorationsEnabled");
        if !matches!(enabled.as_str(), Some("both" | "overviewRuler")) {
            return None;
        }
        let instance = self.terminal_view.instance(id)?;
        let mut marks = Vec::new();
        for (_, line, exit_code) in instance.decorated_commands() {
            let color = match exit_code {
                Some(0) => "terminalCommandDecoration.successBackground",
                Some(_) => "terminalCommandDecoration.errorBackground",
                None => "terminalCommandDecoration.defaultBackground",
            };
            marks.push((line, self.color(color), exit_code.is_some_and(|code| code != 0)));
        }
        let navigation = instance.navigation_highlight.as_ref().map(|highlight| (*highlight.rows.start(), self.color("terminalOverviewRuler.cursorForeground")));
        Some(grid::OverviewRuler { border: self.color("terminalOverviewRuler.border"), marks, navigation })
    }

    /// quickFixAddon.ts `_registerQuickFixDecoration`: a light bulb (a sparkle when every fix explains) over the gutter
    /// of the line the cursor was on when the command finished, opening the fixes on a click.
    fn terminal_light_bulb(&self, id: u32, origin: gpui::Point<gpui::Pixels>, viewport_y: i64, rows: i64, scalar: f32, cell_height: f32, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if self.setting("terminal.integrated.shellIntegration.quickFixEnabled") == json!(false) {
            return None;
        }
        let shown = self.terminal_view.quick_fixes.shown.get(&id)?;
        let row = shown.line - viewport_y;
        if !(0..rows).contains(&row) {
            return None;
        }
        let explain_only = shown.fixes.iter().all(|fix| fix.explain);
        let (icon, color) = if explain_only { ("sparkle", "editorLightBulbAutoFix.foreground") } else { ("lightbulb", "editorLightBulb.foreground") };
        let size = scalar * DEFAULT_DIMENSION;
        let background = self.theme.color("terminal.background").unwrap_or_else(|| self.color("panel.background"));
        let element = self
            .icon(icon, size, size, self.color(color))
            .id(("terminal-quick-fix", id as usize))
            .absolute()
            .left(origin.x + px(scalar * MARGIN_LEFT))
            .top(origin.y + px(row as f32 * cell_height))
            .bg(background)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let at = gpui::point(f32::from(event.position.x), f32::from(event.position.y));
                workbench.show_terminal_quick_fixes(Some(at), window, cx);
            }));
        Some(element.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hover_tells_when_the_command_ran_how_long_and_its_exit_code() {
        let failed = crate::terminal::FinishedCommand { timestamp: 100_000, duration: 1234, exit_code: Some(2), ..Default::default() };
        assert_eq!(decoration_hover(&failed, 105_000), "Show Command Actions\n\n---\n\nCommand executed now, took 1.234s and failed (Exit Code 2)");
        let passed = crate::terminal::FinishedCommand { timestamp: 100_000, duration: 120_000, exit_code: Some(0), ..Default::default() };
        assert_eq!(decoration_hover(&passed, 400_000), "Show Command Actions\n\n---\n\nCommand executed 5 mins ago and took 2 mins");
        assert_eq!(duration_string(999), "999ms");
        assert_eq!(duration_string(2000), "2s");
    }

    #[gpui::test]
    fn toggle_visibility_writes_the_decorations_setting(cx: &mut gpui::TestAppContext) {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let user = std::env::temp_dir().join(format!("code-gpui-decoration-visibility-{}", std::process::id()));
        std::fs::create_dir_all(&user).unwrap();
        let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), user.join("User"), None)));
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        workbench.update(cx, |workbench, _| {
            for (handles, expected) in [(vec![1], "overviewRuler"), (vec![0], "gutter"), (vec![], "never"), (vec![0, 1], "both")] {
                workbench.decoration_visibility_event(Event::ChangeSelection(handles));
                assert_eq!(workbench.setting("terminal.integrated.shellIntegration.decorationsEnabled"), json!(expected));
            }
        });
    }
}
