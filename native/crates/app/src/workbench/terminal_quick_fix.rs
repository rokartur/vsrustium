//! terminal.quickFix.contribution.ts, quickFixAddon.ts and terminalQuickFixService.ts: after a command finishes,
//! the builtin fixes and those of extension providers (`contributes.terminalQuickFixes`, `$registerQuickFixProvider`)
//! show as a light bulb at the new prompt, in the command decoration's menu and in the action widget
//! (Cmd+. `workbench.action.terminal.showQuickFixes`).

use std::rc::Rc;

use gpui::{Context, Window};
use regex::Regex;
use serde_json::{Value, json};

use super::Workbench;
use super::action_widget::Anchor;
use crate::action_list::{Item, ItemKind};
use crate::commands::Outcome;
use crate::terminal::quick_fix::{Fix, OutputMatch, OutputMatcher, QuickFix, dedup, exit_result_matches};

/// `MAX_LENGTH` of mainThreadTerminalService.ts `$registerQuickFixProvider`: an extension reads at most 40 lines.
const MAX_EXTENSION_LINES: usize = 40;

#[derive(Default)]
pub(super) struct TerminalQuickFixes {
    /// `$registerQuickFixProvider`: provider id and its extension.
    providers: Vec<(String, String)>,
    /// The fixes of each instance's last command: its index in `commands`, the buffer line of the light bulb.
    pub(super) shown: std::collections::HashMap<u32, Shown>,
    /// How many `_terminal.runQuickFix.<n>` commands the decoration menu has registered.
    menu_commands: usize,
}

pub(super) struct Shown {
    pub command: usize,
    pub line: i64,
    pub fixes: Rc<Vec<QuickFix>>,
}

/// A `contributes.terminalQuickFixes` entry (`ITerminalCommandSelector`).
struct Selector {
    id: String,
    command_line: Regex,
    output: OutputMatcher,
    error: bool,
    explain: bool,
}

impl Workbench {
    pub(super) fn register_terminal_quick_fix_actions(&mut self) {
        self.commands.register("workbench.action.terminal.showQuickFixes", |workbench, _, window, cx| {
            workbench.show_terminal_quick_fixes(None, window, cx);
            Outcome::done()
        });
    }

    pub(super) fn handle_quick_fix_provider_request(&mut self, method: &str, args: &[Value]) {
        let id = args.first().and_then(Value::as_str).unwrap_or_default().to_owned();
        match method {
            "$registerQuickFixProvider" => self.terminal_view.quick_fixes.providers.push((id, args.get(1).and_then(Value::as_str).unwrap_or_default().to_owned())),
            _ => self.terminal_view.quick_fixes.providers.retain(|(known, _)| *known != id),
        }
    }

    /// `_resolveQuickFixes` on `onCommandFinished`: a command with a command line clears the last fixes, then the
    /// builtin fixes and the providers' answers, if any, replace them.
    pub(super) fn terminal_command_finished_quick_fixes(&mut self, id: u32, cx: &mut Context<Self>) {
        let selectors = self.terminal_quick_fix_selectors();
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let Some(command) = instance.commands.last().cloned() else { return };
        let index = instance.commands.len() - 1;
        // `registerMarker()`: the light bulb goes on the cursor's line, the next prompt.
        let line = instance.term.grid().cursor.point.line.0 as i64 + alacritty_terminal::grid::Dimensions::history_size(&instance.term) as i64;
        let builtin = instance.builtin_quick_fixes(&command);
        let mut requests = Vec::new();
        for selector in selectors {
            if !exit_result_matches(selector.error, command.exit_code) {
                continue;
            }
            let Some(lines) = instance.lines_for_command(&command, &selector.output) else { continue };
            requests.push((selector, lines));
        }
        if !command.command.is_empty() {
            self.terminal_view.quick_fixes.shown.remove(&id);
        }
        let activations: Vec<_> = requests.iter().map(|(selector, _)| self.host.activate_by_event(&format!("onTerminalQuickFixRequest:{}", selector.id))).collect();
        let command_line = command.command.clone();
        cx.spawn(async move |workbench, cx| {
            futures::future::join_all(activations).await;
            let mut fixes = builtin;
            for (selector, lines) in requests {
                let Ok(Some((call, extension))) = workbench.update(cx, |workbench, _| {
                    let (_, extension) = workbench.terminal_view.quick_fixes.providers.iter().find(|(provider, _)| *provider == selector.id)?.clone();
                    let command_match = selector.command_line.captures(&command_line)?;
                    let output = OutputMatch::from_captures(&selector.output.line_matcher, &lines.join("\n"), lines.clone())?;
                    let match_result = json!({
                        "commandLine": command_line,
                        "commandLineMatch": command_match.iter().map(|group| group.map(|group| group.as_str())).collect::<Vec<_>>(),
                        "outputMatch": { "regexMatch": output.groups, "outputLines": output.lines },
                    });
                    Some((workbench.host.call("ExtHostTerminalService", "$provideTerminalQuickFixes", vec![json!(selector.id), match_result]), extension))
                }) else {
                    continue;
                };
                let Ok(Ok(answer)) = call.await else { continue };
                let answers = match answer {
                    Value::Array(answers) => answers,
                    Value::Null => Vec::new(),
                    answer => vec![answer],
                };
                fixes.extend(answers.iter().filter_map(|answer| parse_extension_fix(&selector.id, &extension, selector.explain, answer)));
            }
            let fixes = dedup(fixes);
            if fixes.is_empty() {
                return;
            }
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.terminal_view.quick_fixes.shown.insert(id, Shown { command: index, line, fixes: Rc::new(fixes) });
                cx.notify();
            });
        })
        .detach();
    }

    /// `contributes.terminalQuickFixes` of the running extensions, an output matcher capped at 40 lines.
    fn terminal_quick_fix_selectors(&self) -> Vec<Selector> {
        let mut selectors = Vec::new();
        for extension in &self.extensions {
            for entry in extension.description["contributes"]["terminalQuickFixes"].as_array().into_iter().flatten() {
                let output = &entry["outputMatcher"];
                let (Some(id), Some(command_line), Some(line_matcher)) = (entry["id"].as_str(), entry["commandLineMatcher"].as_str(), output["lineMatcher"].as_str()) else { continue };
                let (Ok(command_line), Ok(line_matcher)) = (Regex::new(command_line), Regex::new(line_matcher)) else { continue };
                selectors.push(Selector {
                    id: id.to_owned(),
                    command_line,
                    output: OutputMatcher {
                        line_matcher,
                        anchor_bottom: output["anchor"] == "bottom",
                        offset: output["offset"].as_i64().unwrap_or(0),
                        length: (output["length"].as_u64().unwrap_or(1) as usize).min(MAX_EXTENSION_LINES),
                    },
                    error: entry["commandExitResult"] == "error",
                    explain: entry["kind"] == "explain",
                });
            }
        }
        selectors
    }

    /// `showMenu`: the action widget with a Quick Fix header and each fix, at the light bulb or (from the keyboard)
    /// at the cursor.
    pub(super) fn show_terminal_quick_fixes(&mut self, at: Option<gpui::Point<f32>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.terminal_view.active_instance_id() else { return };
        let Some(shown) = self.terminal_view.quick_fixes.shown.get(&id) else { return };
        let fixes = shown.fixes.clone();
        let anchor = at.or_else(|| self.terminal_light_bulb_point(id, shown.line)).unwrap_or_default();
        let mut items = vec![Item { label: Some("Quick Fix".into()), ..Item::new(ItemKind::Header) }];
        let mut looks = vec![super::code_actions::RowLook { icon: "", color: None, keybinding: None }];
        for (index, fix) in fixes.iter().enumerate() {
            items.push(Item::action(index, fix.label()));
            looks.push(super::code_actions::RowLook { icon: fix.icon(), color: None, keybinding: None });
        }
        self.show_terminal_quick_fix_widget(items, looks, Anchor::Point(anchor), id, fixes, cx);
        window.focus(&self.terminal_view.focus, cx);
    }

    /// The light bulb's window point: the left of the pane at the line's row.
    fn terminal_light_bulb_point(&self, id: u32, line: i64) -> Option<gpui::Point<f32>> {
        let (instance, pane, metrics) = (self.terminal_view.instance(id)?, self.terminal_view.pane_bounds.get(&id)?, self.terminal_view.metrics.as_ref()?);
        let row = line - instance.viewport_y();
        Some(gpui::point(f32::from(pane.origin.x), f32::from(pane.origin.y) + (row + 1) as f32 * f32::from(metrics.cell_height)))
    }

    /// An accepted fix: a command line goes to the terminal, a URI opens, a port is freed, a command runs.
    pub(super) fn run_terminal_quick_fix(&mut self, id: u32, fix: &QuickFix, window: &mut Window, cx: &mut Context<Self>) {
        match &fix.fix {
            Fix::Command { command, execute } => self.run_terminal_command(id, command.clone(), *execute, window, cx),
            Fix::Open(uri) => {
                let outcome = self.execute_command("vscode.open", vec![json!(uri)], window, cx);
                super::quick_access::show_failure(fix.label(), outcome, window, cx);
            }
            Fix::FreePort(port) => self.free_port(id, port, window, cx),
            Fix::VscodeCommand { id: command, .. } => drop(self.execute_command(command, Vec::new(), window, cx)),
        }
    }

    /// terminalInstance.ts `freePortKillProcess` and ptyService.ts: the first process `lsof` lists listening on the
    /// port is killed, then the command line is typed again without running it.
    fn free_port(&mut self, id: u32, port: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command_line) = self.terminal_view.quick_fixes.shown.get(&id).and_then(|shown| self.terminal_view.instance(id)?.commands.get(shown.command)).map(|command| command.command.clone()) else { return };
        let port = port.to_owned();
        let killed = cx.background_executor().spawn(async move {
            let listing = std::process::Command::new("lsof").args(["-nP", "-iTCP", "-sTCP:LISTEN"]).output().ok()?;
            let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
            let line = listing.lines().find(|line| line.contains(&port) && !line.trim().is_empty())?;
            // ptyService.ts `capturePid`: the first number with blanks around it.
            let pid: i32 = line.split_whitespace().find_map(|field| field.parse().ok())?;
            // SAFETY: kill(2) with a pid and SIGTERM has no memory effects.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            Some(())
        });
        cx.spawn_in(window, async move |workbench, cx| {
            killed.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.run_terminal_command(id, command_line, false, window, cx));
        })
        .detach();
    }

    /// `registerMenuItems`: the decoration menu of the command with fixes lists them first, each through a
    /// `_terminal.runQuickFix.<n>` command taking the instance and command index the menu passes.
    pub(super) fn terminal_quick_fix_menu_actions(&mut self, id: u32, command: usize) -> Vec<crate::menus::MenuAction> {
        let Some(fixes) = self.terminal_view.quick_fixes.shown.get(&id).filter(|shown| shown.command == command).map(|shown| shown.fixes.clone()) else { return Vec::new() };
        while self.terminal_view.quick_fixes.menu_commands < fixes.len() {
            let fix_index = self.terminal_view.quick_fixes.menu_commands;
            self.commands.register(format!("_terminal.runQuickFix.{fix_index}"), move |workbench, args, window, cx| {
                let Some(id) = args.first().and_then(Value::as_u64).map(|id| id as u32) else { return Outcome::done() };
                let Some(fix) = workbench.terminal_view.quick_fixes.shown.get(&id).and_then(|shown| shown.fixes.get(fix_index).cloned()) else { return Outcome::done() };
                workbench.run_terminal_quick_fix(id, &fix, window, cx);
                Outcome::done()
            });
            self.terminal_view.quick_fixes.menu_commands += 1;
        }
        fixes.iter().enumerate().map(|(index, fix)| crate::menus::MenuAction::Command { id: format!("_terminal.runQuickFix.{index}"), title: fix.label(), enabled: true, checked: false, alt: None }).collect()
    }
}

/// mainThreadTerminalService.ts `parseQuickFix` and `getQuickFixesForCommand`: a `uri` opens, an `id` is a
/// command, anything else is a terminal command.
fn parse_extension_fix(id: &str, extension: &str, explain: bool, answer: &Value) -> Option<QuickFix> {
    let fix = if let Some(uri) = answer.get("uri") {
        Fix::Open(crate::uri::Uri::from_json(uri).ok()?.to_string(false))
    } else if let Some(command) = answer.get("id").and_then(Value::as_str) {
        Fix::VscodeCommand { id: command.to_owned(), title: answer["title"].as_str().unwrap_or(command).to_owned() }
    } else {
        Fix::Command { command: answer["terminalCommand"].as_str()?.to_owned(), execute: answer["shouldExecute"].as_bool().unwrap_or(true) }
    };
    Some(QuickFix { id: id.to_owned(), source: extension.to_owned(), explain, fix })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_answers_become_fixes_by_their_shape() {
        let fix = |answer: Value| parse_extension_fix("ms-vscode.npm-command", "vscode.npm", false, &answer).map(|fix| fix.label());
        assert_eq!(fix(json!({ "terminalCommand": "npm run build" })).as_deref(), Some("Run: npm run build"));
        assert_eq!(fix(json!({ "id": "workbench.action.reloadWindow", "title": "Reload" })).as_deref(), Some("Reload"));
        assert_eq!(fix(json!({ "uri": { "scheme": "https", "authority": "x.y", "path": "/z" } })).as_deref(), Some("Open: https://x.y/z"));
    }

    /// A failed `git push` without upstream: the fix shows in the widget (Cmd+.) and accepting it runs the command.
    #[gpui::test]
    fn a_finished_command_offers_its_fixes_in_the_action_widget(cx: &mut gpui::TestAppContext) {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx));
        let written = Rc::new(std::cell::RefCell::new(String::new()));
        let sink = written.clone();
        workbench.update_in(cx, |workbench, window, cx| {
            let launch = crate::terminal::ShellLaunchConfig { extension_pty: true, cwd: std::env::temp_dir(), ..Default::default() };
            let id = workbench.add_terminal(launch, None, window, cx);
            workbench.terminal_view.instance_mut(id).unwrap().process_input = Some(Box::new(move |data: &str| sink.borrow_mut().push_str(data)));
            let output = "fatal: The current branch feat has no upstream branch.\r\n\r\n    git push --set-upstream origin feat\r\n\r\n";
            let data = format!("\x1b]633;A\x07$ \x1b]633;B\x07git push\r\n\x1b]633;E;git push\x07\x1b]633;C\x07{output}\x1b]633;D;128\x07\x1b]633;A\x07$ \x1b]633;B\x07");
            workbench.terminal_data(id, data.as_bytes(), window, cx);
        });
        cx.run_until_parked();
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.terminal.showQuickFixes", Vec::new(), window, cx)));
        let labels = workbench.read_with(cx, |workbench, _| workbench.action_widget_labels());
        assert_eq!(labels, ["Quick Fix", "Run: git push --set-upstream origin feat"]);
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("acceptSelectedCodeAction", Vec::new(), window, cx)));
        cx.executor().advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        assert_eq!(*written.borrow(), "git push --set-upstream origin feat\r");
        assert!(!workbench.read_with(cx, |workbench, _| workbench.action_widget_visible()));
    }
}
