//! output.contribution.ts actions of the Output view: switch output, Show Output Channels, clear, auto scroll, open
//! in editor, the active channel's log level, the level and category filters and the filter text.

use std::collections::HashMap;

use gpui::{Bounds, Context, Pixels, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::output_view::OUTPUT_VIEW_ID;
use crate::commands::Outcome;
use crate::input_box::InputBox;
use crate::main_thread_quick_open::Owner;
use crate::menus::MenuAction;
use crate::output::filter::{Level, parse_log_entries};
use crate::output::{DEFAULT_LOG_LEVEL, LOG_LEVELS, OUTPUT_SCHEME, OutputChannel, WINDOW_LOG_ID};
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

/// What accepting an item of an output channel picker does.
#[derive(Clone, Debug)]
pub enum OutputPick {
    /// `IOutputService.showChannel`.
    Show(String),
    /// `editorService.openEditor({ resource: channel.uri, options: { pinned: true } })`.
    OpenFile(String),
}

/// The level toggles of the filter menu, with `LogLevelToString` and `LogLevelToLocalizedString`.
const LEVEL_FILTERS: [(Level, &str, &str); 5] = [(Level::Trace, "trace", "Trace"), (Level::Debug, "debug", "Debug"), (Level::Info, "info", "Info"), (Level::Warning, "warn", "Warning"), (Level::Error, "error", "Error")];

impl Workbench {
    pub(super) fn register_output_actions(&mut self) {
        // viewsService.ts `OpenViewAction` of the view's `openCommandActionDescriptor`.
        self.commands.register("workbench.action.output.toggleOutput", |workbench, args, window, cx| {
            let preserve_focus = args.first().is_some_and(|options| options["preserveFocus"] == true);
            if workbench.output_view_visible() && workbench.editor_focus.is_focused(window) && !preserve_focus {
                workbench.close_output_view(cx);
            } else {
                workbench.open_output_view(!preserve_focus, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.output.action.switchBetweenOutputs", |workbench, args, window, cx| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                workbench.show_output_channel(id, true, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.showOutputChannels", |workbench, _, window, cx| {
            workbench.show_output_channels_picker(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.output.action.clearOutput", |workbench, _, _, cx| {
            if let Some(id) = workbench.output.active.clone() {
                workbench.update_output_channel(&id, 3, None, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.output.action.toggleAutoScroll", |workbench, _, _, cx| {
            workbench.output_view.scroll_lock = !workbench.output_view.scroll_lock;
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.openActiveLogOutputFile", |workbench, _, window, cx| {
            if let Some(id) = workbench.output.active.clone() {
                workbench.accept_output_pick(OutputPick::OpenFile(id), window, cx);
            }
            Outcome::done()
        });
        for (level, _, _) in LOG_LEVELS {
            self.commands.register(format!("workbench.action.output.activeOutputLogLevel.{level}"), move |workbench, _, _, _| {
                if let Some(id) = workbench.output.active.clone() {
                    workbench.set_output_log_level(&id, level);
                }
                Outcome::done()
            });
        }
        for (level, name, _) in LEVEL_FILTERS {
            self.commands.register(format!("workbench.actions.{OUTPUT_VIEW_ID}.toggle.{name}"), move |workbench, _, _, cx| {
                let shown = workbench.output.filters.level_mut(level);
                *shown = !*shown;
                workbench.output_filters_changed(cx);
                Outcome::done()
            });
        }
        self.commands.register(format!("workbench.actions.{OUTPUT_VIEW_ID}.clearFilterText"), |workbench, _, window, cx| {
            workbench.edit_output_filter(|input| input.set_value(String::new()), window, cx);
            Outcome::done()
        });
    }

    /// logService.ts registers the Window logger before the workbench's contributions run.
    pub fn register_window_log(&mut self, file: std::path::PathBuf) {
        self.output.register_window_log(file);
        self.sync_output_channel_commands();
    }

    /// `registerSwitchOutputAction`: a `workbench.action.output.show.<id>` command for every channel.
    pub(super) fn sync_output_channel_commands(&mut self) {
        let ids: Vec<String> = self.output.channels.iter().map(|channel| channel.id.clone()).collect();
        for removed in self.output_view.channel_commands.iter().filter(|id| !ids.contains(id)) {
            self.commands.unregister(&format!("workbench.action.output.show.{removed}"));
        }
        for added in ids.iter().filter(|id| !self.output_view.channel_commands.contains(id)) {
            let id = added.clone();
            self.commands.register(format!("workbench.action.output.show.{added}"), move |workbench, _, window, cx| {
                workbench.show_output_channel(&id, true, window, cx);
                Outcome::done()
            });
        }
        self.output_view.channel_commands = ids;
    }

    /// outputServices.ts context keys of the active channel and the view's filters.
    pub(super) fn output_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        context.insert(format!("{OUTPUT_VIEW_ID}.active"), json!(true));
        context.insert("inOutput".into(), json!(self.output_view_visible() && self.editor_focus.is_focused(window)));
        context.insert("outputView.scrollLock".into(), json!(self.output_view.scroll_lock));
        context.insert("outputFilterFocus".into(), json!(self.output_view.filter_focus.is_focused(window)));
        let filters = &self.output.filters;
        for (level, name, _) in LEVEL_FILTERS {
            let key = if level == Level::Warning { "warning" } else { name };
            context.insert(format!("output.filter.{key}"), json!(filters.level_shown(level)));
        }
        context.insert("output.filter.categories".into(), json!(filters.categories));
        let Some(channel) = self.output.active_channel() else { return };
        context.insert("activeOutputChannel".into(), json!(channel.id));
        context.insert("activeLogOutput".into(), json!(true));
        context.insert("activeLogOutput.isLog".into(), json!(channel.log));
        let level = self.output.channel_log_level(&channel.id);
        context.insert("activeLogOutput.levelSettable".into(), json!(channel.log));
        context.insert("activeLogOutput.level".into(), json!(level.map_or("", level_name)));
        context.insert("activeLogOutput.levelIsDefault".into(), json!(level == Some(DEFAULT_LOG_LEVEL)));
    }

    /// The channel dropdown (`isSelection` submenu `workbench.output.menu.switchOutput`): extension channels, then
    /// the port's own, the active one checked.
    pub(super) fn show_output_channel_menu(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mut groups: [Vec<&OutputChannel>; 2] = [Vec::new(), Vec::new()];
        for channel in &self.output.channels {
            groups[usize::from(channel.extension_id.is_none())].push(channel);
        }
        let mut actions = Vec::new();
        for group in groups.iter_mut().filter(|group| !group.is_empty()) {
            group.sort_by(|a, b| a.label.cmp(&b.label));
            if !actions.is_empty() {
                actions.push(MenuAction::Separator);
            }
            for channel in group.iter() {
                let checked = self.output.active.as_deref() == Some(channel.id.as_str());
                actions.push(MenuAction::Command { id: format!("workbench.action.output.show.{}", channel.id), title: channel.label.clone(), enabled: true, checked, alt: None });
            }
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// `viewFilterSubmenu`: the categories of the shown log entries, then the level toggles of a log channel.
    pub(super) fn show_output_filter_menu(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.output.active_channel() else { return };
        let (channel_id, log) = (channel.id.clone(), channel.log);
        let lines: Vec<String> = self.output_view.text.split('\n').map(str::to_owned).collect();
        let mut categories: Vec<String> = Vec::new();
        for category in parse_log_entries(&lines).into_iter().filter_map(|entry| entry.category) {
            if !categories.contains(&category) {
                categories.push(category);
            }
        }
        let mut actions = Vec::new();
        for category in categories {
            let filter = format!("{channel_id}:{category}");
            let command = format!("workbench.actions.{OUTPUT_VIEW_ID}.toggle.{filter}");
            if self.commands.get(&command).is_none() {
                let toggled = filter.clone();
                self.commands.register(command.clone(), move |workbench, _, _, cx| {
                    workbench.output.filters.toggle_category(&toggled);
                    workbench.output_filters_changed(cx);
                    Outcome::done()
                });
            }
            actions.push(MenuAction::Command { id: command, title: category, enabled: true, checked: !self.output.filters.has_category(&filter), alt: None });
        }
        if log {
            if !actions.is_empty() {
                actions.push(MenuAction::Separator);
            }
            for (level, name, title) in LEVEL_FILTERS {
                let checked = self.output.filters.level_shown(level);
                actions.push(MenuAction::Command { id: format!("workbench.actions.{OUTPUT_VIEW_ID}.toggle.{name}"), title: title.into(), enabled: true, checked, alt: None });
            }
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// The gear's submenu `workbench.output.menu.logLevel`: the levels, the active channel's checked.
    // shortcut: no "Set As Default" item; default log levels (argv.json `log-level`) are milestone 15's logs item.
    pub(super) fn show_output_log_level_menu(&mut self, anchor: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.output.active.clone() else { return };
        let current = self.output.channel_log_level(&id);
        let actions = LOG_LEVELS
            .iter()
            .map(|&(level, _, title)| MenuAction::Command { id: format!("workbench.action.output.activeOutputLogLevel.{level}"), title: title.into(), enabled: true, checked: current == Some(level), alt: None })
            .collect();
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// `OutputService.setLogLevel`: the logger's level changes and `MainThreadLogger` tells the host
    /// (`onDidChangeLogLevel`).
    fn set_output_log_level(&mut self, id: &str, level: u8) {
        let Some(file) = self.output.set_channel_log_level(id, level) else { return };
        if id != WINDOW_LOG_ID {
            self.host.send("ExtHostLogLevelServiceShape", "$setLogLevel", vec![json!(level), exthost::scan::file_uri(&file)]);
        }
    }

    /// `registerShowOutputChannelsAction`.
    fn show_output_channels_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (extension, core): (Vec<&OutputChannel>, Vec<&OutputChannel>) = self.output.channels.iter().partition(|channel| channel.extension_id.is_some());
        let mut rows = Vec::new();
        rows.extend(extension.iter().map(|channel| output_row(channel, OutputPick::Show)));
        if !extension.is_empty() && !core.is_empty() {
            rows.push(None);
        }
        rows.extend(core.iter().map(|channel| output_row(channel, OutputPick::Show)));
        self.show_output_picker("Select Output Channel", rows, window, cx);
    }

    /// A quick pick of channels; `None` rows are unlabelled separators.
    pub(super) fn show_output_picker(&mut self, placeholder: &str, rows: Vec<Option<(String, OutputPick)>>, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries = Vec::new();
        let mut picks = Vec::new();
        for row in rows {
            let Some((label, pick)) = row else {
                entries.push(Entry::Separator(None));
                continue;
            };
            entries.push(Entry::Item(Item { handle: picks.len() as i64, label, pickable: true, ..Item::default() }));
            picks.push(pick);
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some(placeholder.into());
        input.set_items(entries);
        let id = self.quick_inputs.add(input, Owner::Output(picks));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn output_pick_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let Some(Owner::Output(picks)) = self.quick_inputs.owner(id) else { return };
                let pick = picks[handle as usize].clone();
                self.hide_quick_input_by_id(id, window, cx);
                self.accept_output_pick(pick, window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    // shortcut: opens the channel's file, where upstream opens its `output:` resource in a read-only text editor; the
    // port's group hosts one output-scheme editor, the Output view.
    fn accept_output_pick(&mut self, pick: OutputPick, window: &mut Window, cx: &mut Context<Self>) {
        match pick {
            OutputPick::Show(id) => self.show_output_channel(&id, false, window, cx),
            OutputPick::OpenFile(id) => {
                let Some(channel) = self.output.channel(&id) else { return };
                self.open_file(channel.file.clone(), true, window, cx);
            }
        }
    }

    /// `FilterWidget.onDidChangeFilterText` after its 300 ms delay: `filters.text` changes and the view refilters.
    pub(super) fn edit_output_filter(&mut self, edit: impl FnOnce(&mut InputBox), window: &mut Window, cx: &mut Context<Self>) {
        let before = self.output_view.filter.value.clone();
        edit(&mut self.output_view.filter);
        cx.notify();
        if self.output_view.filter.value == before {
            return;
        }
        self.output_view.filtering = Some(cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let text = workbench.output_view.filter.value.clone();
                workbench.output.filters.set_text(&text);
                workbench.output_filters_changed(cx);
            });
        }));
    }

    /// outputView.ts `OutputEditor.onDidChangeCursorPosition`: with `output.smartScroll.enabled`, a cursor on the
    /// last line turns auto scrolling on and any other line turns it off.
    pub(super) fn output_cursor_moved(&mut self) {
        let Some(editor) = self.editors.active().filter(|editor| editor.document.scheme == OUTPUT_SCHEME) else { return };
        if self.setting("output.smartScroll.enabled") != true {
            return;
        }
        let last_line = editor.document.lines.len() - 1;
        self.output_view.scroll_lock = editor.selections[0].head.line != last_line;
    }
}

fn output_row(channel: &OutputChannel, pick: fn(String) -> OutputPick) -> Option<(String, OutputPick)> {
    Some((channel.label.clone(), pick(channel.id.clone())))
}

/// log.ts `LogLevelToString`.
fn level_name(level: u8) -> &'static str {
    LOG_LEVELS.iter().find(|(known, _, _)| *known == level).expect("a log level").1
}
