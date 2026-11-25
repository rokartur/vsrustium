//! Accessibility of the workbench: IAccessibilityService (platform/accessibility, the electron-browser
//! accessibilityService.ts of workbench/services/accessibility) on GPUI's AccessKit tree, the screen reader
//! status entry and prompt (accessibilityStatus.ts), Toggle Screen Reader Accessibility Mode (codeEditor
//! accessibility.ts), the aria live region (base/browser/ui/aria/aria.ts `alert` and `status`), the signals
//! (accessibilitySignalService.ts, editorTextPropertySignalsContribution.ts, saveAccessibilitySignal.ts) and their
//! help pickers (accessibilitySignals/browser/commands.ts), and what the editor and the Explorer expose to VoiceOver.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{A11ySubtreeBuilder, Context, Role, Task, Window, accesskit, div, prelude::*};
use serde_json::{Value, json};

use super::Workbench;
use crate::accessibility_signals::{Modality, SIGNALS, SoundPlayer, is_enabled, signal_by_key, volume_percent};
use crate::commands::Outcome;
use crate::editor::{OpenEditor, Position};
use crate::main_thread_quick_open::Owner;
use crate::main_thread_status_bar::StatusEntry;
use crate::markers::{ERROR, ReadOptions, WARNING};
use crate::notifications::Severity;
use crate::quick_input::{Button, Event, Item, Kind, QuickInput};

const SCREEN_READER_STATUS_ID: &str = "status.editor.screenReaderMode";
/// Signals played as a user gesture by default (`userGestureSignals` of commands.ts).
const USER_GESTURE_SIGNALS: [&str; 2] = ["accessibility.signals.save", "accessibility.signals.format"];
/// editorTextPropertySignalsContribution.ts: signals wait this long more right after typing.
const TYPING_DELAY: Duration = Duration::from_millis(1000);

pub struct AccessibilityState {
    sounds: SoundPlayer,
    /// `accessibilitySupportChanged` from the platform: assistive technology reads the AccessKit tree.
    screen_reader_attached: bool,
    prompted: bool,
    /// The live region's message and a counter that gives each message its own node, so a repeat is read again.
    alert: Option<(String, u64)>,
    /// editorTextPropertySignalsContribution.ts per model: the cursor last seen, `lastLine`,
    /// `ignoredLineSignalsForCurrentLine` (by line signal), the version seen with the time it changed (`didType`),
    /// the signal timeouts, and what each watched property had at the cursor (at the position, on the line).
    cursor: Option<(u32, Position)>,
    line: Option<(u32, usize)>,
    ignored_line_signals: HashSet<&'static str>,
    version: (u32, u32, Instant),
    pending: Vec<Task<()>>,
    watched: Vec<(bool, bool)>,
    pub view: super::accessible_view::AccessibleViewState,
}

impl AccessibilityState {
    pub fn new(app_root: &Path, cx: &mut Context<Workbench>) -> Self {
        let view = super::accessible_view::AccessibleViewState::new(cx);
        AccessibilityState { sounds: SoundPlayer::new(app_root), screen_reader_attached: false, prompted: false, alert: None, cursor: None, line: None, ignored_line_signals: HashSet::new(), version: (0, 0, Instant::now()), pending: Vec::new(), watched: Vec::new(), view }
    }
}

/// A `TextProperty`: its line signal, its position signal, and the marker severity it looks for (none for folding).
#[derive(Clone, Copy)]
struct TextProperty {
    line_signal: &'static str,
    position_signal: Option<&'static str>,
    severity: Option<u8>,
}

#[derive(Clone, Copy, PartialEq)]
enum SignalMode {
    Line,
    Position,
}

/// `MarkerTextProperty` for errors and warnings and `FoldedAreaTextProperty`, in upstream's order. shortcut:
/// `BreakpointTextProperty` waits for the debugger's breakpoints (milestone 16).
const TEXT_PROPERTIES: [TextProperty; 3] = [
    TextProperty { line_signal: "accessibility.signals.lineHasError", position_signal: Some("accessibility.signals.positionHasError"), severity: Some(ERROR) },
    TextProperty { line_signal: "accessibility.signals.lineHasWarning", position_signal: Some("accessibility.signals.positionHasWarning"), severity: Some(WARNING) },
    TextProperty { line_signal: "accessibility.signals.lineHasFoldedArea", position_signal: None, severity: None },
];

impl Workbench {
    pub(super) fn register_accessibility_actions(&mut self) {
        self.commands.register("editor.action.toggleScreenReaderAccessibilityMode", |workbench, _, _, cx| {
            let optimized = workbench.is_screen_reader_optimized();
            workbench.update_user_setting("editor.accessibilitySupport", json!(if optimized { "off" } else { "on" }));
            // AccessibilityHelpNLS.screenReaderModeDisabled and screenReaderModeEnabled.
            workbench.aria_alert(if optimized { "Screen Reader Optimized Mode disabled" } else { "Screen Reader Optimized Mode enabled" });
            workbench.screen_reader_mode_changed(cx);
            Outcome::done()
        });
        self.commands.register("showEditorScreenReaderNotification", |workbench, _, _, cx| {
            workbench.show_screen_reader_notification(cx);
            Outcome::done()
        });
        self.commands.register("signals.sounds.help", |workbench, _, window, cx| {
            workbench.show_signal_help(Modality::Sound, window, cx);
            Outcome::done()
        });
        self.commands.register("accessibility.announcement.help", |workbench, _, window, cx| {
            workbench.show_signal_help(Modality::Announcement, window, cx);
            Outcome::done()
        });
        self.register_accessible_view_actions();
    }

    /// unfocusedViewDimmingContribution.ts: the opacity of a text editor and its breadcrumbs while the editor does not
    /// hold the focus, with `accessibility.dimUnfocused.enabled`.
    pub(super) fn unfocused_editor_opacity(&self, window: &Window) -> f32 {
        let find = &self.find;
        let focus_within = self.editor_focus.is_focused(window) || self.sticky_scroll.is_focused(window) || find.find_focus.is_focused(window) || find.replace_focus.is_focused(window) || find.button_focus.values().any(|focus| focus.is_focused(window));
        if focus_within || self.setting("accessibility.dimUnfocused.enabled") != json!(true) {
            return 1.;
        }
        self.setting("accessibility.dimUnfocused.opacity").as_f64().map_or(0.75, |opacity| opacity.clamp(0.2, 1.)) as f32
    }

    /// `isScreenReaderOptimized`: `editor.accessibilitySupport` on, or auto with a screen reader attached.
    pub(crate) fn is_screen_reader_optimized(&self) -> bool {
        match self.setting("editor.accessibilitySupport").as_str() {
            Some("on") => true,
            Some("off") => false,
            _ => self.accessibility.screen_reader_attached,
        }
    }

    pub(super) fn accessibility_context(&self, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("accessibilityModeEnabled".into(), json!(self.is_screen_reader_optimized()));
        self.accessible_view_context(context);
    }

    /// Runs on every render: picks up a screen reader attaching or leaving, and plays the line signals of a
    /// cursor that moved to another line.
    pub(super) fn accessibility_frame(&mut self, window: &Window, cx: &mut Context<Self>) {
        let attached = window.is_a11y_active();
        if attached != self.accessibility.screen_reader_attached {
            self.accessibility.screen_reader_attached = attached;
            self.screen_reader_mode_changed(cx);
        }
        self.line_signals(cx);
    }

    /// accessibilityStatus.ts `onScreenReaderModeChange`: the prompt once per window with `auto`, and the status entry.
    fn screen_reader_mode_changed(&mut self, cx: &mut Context<Self>) {
        let optimized = self.is_screen_reader_optimized();
        if optimized && self.setting("editor.accessibilitySupport") == json!("auto") && !self.accessibility.prompted {
            self.accessibility.prompted = true;
            self.show_screen_reader_notification(cx);
        }
        self.status_entries.retain(|entry| entry.entry_id != SCREEN_READER_STATUS_ID);
        if optimized {
            self.status_entries.push(StatusEntry {
                entry_id: SCREEN_READER_STATUS_ID.into(),
                id: SCREEN_READER_STATUS_ID.into(),
                name: "Screen Reader Mode".into(),
                text: "Screen Reader Optimized".into(),
                tooltip: None,
                command: Some(json!("showEditorScreenReaderNotification")),
                color: None,
                background_color: None,
                kind: Some("prominent"),
                align_left: false,
                priority: 100.6,
                secondary: 0,
                show_progress: false,
            });
        }
        cx.notify();
    }

    fn show_screen_reader_notification(&mut self, cx: &mut Context<Self>) {
        let message = "Screen reader usage detected. Do you want to enable editor.accessibilitySupport to optimize the editor for screen reader usage?";
        let reply = self.prompt_notification(Severity::Info, message, &["Yes", "No", "Learn More"]);
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(choice)) = reply.await else { return };
            let _ = workbench.update(cx, |workbench, cx| match choice.as_u64() {
                Some(0) => workbench.update_user_setting("editor.accessibilitySupport", json!("on")),
                Some(1) => workbench.update_user_setting("editor.accessibilitySupport", json!("off")),
                Some(2) => cx.open_url("https://code.visualstudio.com/docs/editor/accessibility#_screen-readers"),
                _ => {}
            });
        })
        .detach();
    }

    pub(super) fn update_user_setting(&mut self, key: &str, value: Value) {
        let mut settings = self.settings.lock().unwrap();
        match settings.update_value(key, Some(&value), config::service::Target::User, None, &[]) {
            Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
            Ok(_) => {}
            Err(error) => eprintln!("writing {key}: {error}"),
        }
    }

    /// aria.ts `alert`: the live region announces the message.
    pub(crate) fn aria_alert(&mut self, message: &str) {
        let count = self.accessibility.alert.as_ref().map_or(0, |(_, count)| count + 1);
        self.accessibility.alert = Some((message.to_owned(), count));
    }

    /// The live region of aria.ts, an empty element whose synthetic child carries the latest message.
    pub(super) fn aria_live_region(&self) -> impl IntoElement {
        let alert = self.accessibility.alert.clone();
        div().id("aria-container").absolute().size_0().role(Role::Group).a11y_synthetic_children(move |builder| {
            let Some((message, count)) = alert else { return };
            let mut node = accesskit::Node::new(Role::Status);
            node.set_live(accesskit::Live::Assertive);
            node.set_value(message);
            builder.push_child(builder.synthetic_node_id(count), node);
        })
    }

    /// accessibilitySignalService.ts `playSignal`: the announcement through the live region, the sound at the
    /// configured volume.
    pub(crate) fn play_signal(&mut self, settings_key: &str, user_gesture: bool, modality: Option<Modality>) {
        let signal = signal_by_key(settings_key);
        let setting = self.setting(settings_key);
        let attached = self.is_screen_reader_optimized();
        if modality != Some(Modality::Sound) && is_enabled(&setting, Modality::Announcement, attached, user_gesture) {
            if let Some(message) = signal.announcement {
                self.aria_alert(message);
            }
        }
        if modality != Some(Modality::Announcement) && is_enabled(&setting, Modality::Sound, attached, user_gesture) {
            self.play_signal_sound(signal.sounds[0]);
        }
    }

    /// saveAccessibilitySignal.ts: `onDidSave`, a user gesture when the save was explicit.
    pub(super) fn save_signal(&mut self, saved: bool, reason: super::auto_save::SaveReason) {
        if saved {
            self.play_signal("accessibility.signals.save", reason == super::auto_save::SaveReason::Explicit, None);
        }
    }

    fn play_signal_sound(&mut self, file_name: &'static str) {
        let volume = volume_percent(&self.setting("accessibility.signalOptions.volume"));
        self.accessibility.sounds.play(file_name, volume);
    }

    fn signal_enabled(&self, settings_key: &str, modality: Modality) -> bool {
        is_enabled(&self.setting(settings_key), modality, self.is_screen_reader_optimized(), false)
    }

    /// editorTextPropertySignalsContribution.ts `onDidChangeCursorPosition`: a new line triggers each property's
    /// line signal, every move its position signal; while the cursor stays, a property appearing at it triggers too.
    fn line_signals(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else {
            self.accessibility.cursor = None;
            self.accessibility.pending.clear();
            return;
        };
        let position = editor.selections[0].head;
        let (model_id, version) = (editor.model_id, editor.version_id);
        let now = Instant::now();
        let state = &mut self.accessibility;
        if (state.version.0, state.version.1) != (model_id, version) {
            state.version = (model_id, version, now);
        }
        let typed = now.duration_since(state.version.2) < Duration::from_millis(100);
        if state.cursor != Some((model_id, position)) {
            state.cursor = Some((model_id, position));
            state.pending.clear();
            if state.line != Some((model_id, position.line)) {
                state.ignored_line_signals.clear();
                state.line = Some((model_id, position.line));
                for property in TEXT_PROPERTIES {
                    self.trigger_text_property(property, SignalMode::Line, typed, cx);
                }
            }
            for property in TEXT_PROPERTIES {
                self.trigger_text_property(property, SignalMode::Position, typed, cx);
            }
            // The state autoruns start from the current values; upstream returns at the first property with no
            // enabled signal, so the properties after it are not watched.
            let mut watched = Vec::new();
            for property in TEXT_PROPERTIES {
                if !self.any_signal_enabled(property.line_signal) && !property.position_signal.is_some_and(|key| self.any_signal_enabled(key)) {
                    break;
                }
                watched.push(self.text_property_state(property));
            }
            self.accessibility.watched = watched;
            return;
        }
        for index in 0..self.accessibility.watched.len() {
            let property = TEXT_PROPERTIES[index];
            let (was_at_position, was_on_line) = self.accessibility.watched[index];
            let (at_position, on_line) = self.text_property_state(property);
            if !was_at_position && at_position {
                self.trigger_text_property(property, SignalMode::Position, typed, cx);
            }
            if !was_on_line && on_line {
                self.trigger_text_property(property, SignalMode::Line, typed, cx);
            }
            self.accessibility.watched[index] = (at_position, on_line);
        }
    }

    /// `trigger`: each enabled modality after its delay, if the property is still there; a line signal plays once
    /// per line, and a position signal also counts as the line's.
    fn trigger_text_property(&mut self, property: TextProperty, mode: SignalMode, typed: bool, cx: &mut Context<Self>) {
        let key = match mode {
            SignalMode::Line => property.line_signal,
            SignalMode::Position => match property.position_signal {
                Some(key) => key,
                None => return,
            },
        };
        if !self.any_signal_enabled(key) || !self.text_property_present(property, mode) {
            return;
        }
        for modality in [Modality::Sound, Modality::Announcement] {
            if !self.signal_enabled(key, modality) {
                continue;
            }
            let delay = self.signal_delay(key, modality, mode) + if typed { TYPING_DELAY } else { Duration::ZERO };
            let task = cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(delay).await;
                let _ = workbench.update(cx, |workbench, cx| {
                    if !workbench.text_property_present(property, mode) {
                        return;
                    }
                    if mode == SignalMode::Position || !workbench.accessibility.ignored_line_signals.contains(property.line_signal) {
                        workbench.play_signal(key, false, Some(modality));
                    }
                    workbench.accessibility.ignored_line_signals.insert(property.line_signal);
                    cx.notify();
                });
            });
            self.accessibility.pending.push(task);
        }
    }

    /// `getEnabledState(signal, false).value`: any modality is on.
    fn any_signal_enabled(&self, key: &str) -> bool {
        self.signal_enabled(key, Modality::Sound) || self.signal_enabled(key, Modality::Announcement)
    }

    /// accessibilitySignalService.ts `getDelayMs`.
    fn signal_delay(&self, key: &str, modality: Modality, mode: SignalMode) -> Duration {
        if self.setting("accessibility.signalOptions.debouncePositionChanges") != json!(true) {
            return Duration::ZERO;
        }
        let name = match (key, mode) {
            ("accessibility.signals.positionHasError", SignalMode::Position) => "errorAtPosition",
            ("accessibility.signals.positionHasWarning", SignalMode::Position) => "warningAtPosition",
            _ => "general",
        };
        let value = self.setting(&format!("accessibility.signalOptions.experimental.delays.{name}"));
        let field = if modality == Modality::Sound { "sound" } else { "announcement" };
        Duration::from_millis(value[field].as_u64().unwrap_or(0))
    }

    /// The property at the cursor of the active editor: at its position, and on its line.
    fn text_property_state(&self, property: TextProperty) -> (bool, bool) {
        (self.text_property_present(property, SignalMode::Position), self.text_property_present(property, SignalMode::Line))
    }

    /// `TextPropertySource.isPresent` at the active editor's cursor.
    fn text_property_present(&self, property: TextProperty, mode: SignalMode) -> bool {
        let Some(editor) = self.editors.active() else { return false };
        let position = editor.selections[0].head;
        let (line_number, column) = (position.line + 1, position.column + 1);
        let Some(severity) = property.severity else {
            let folding = &editor.folding;
            return mode == SignalMode::Line && folding.region_at_line(line_number).is_some_and(|index| folding.regions.is_collapsed(index) && folding.regions.get(index).start == line_number);
        };
        let resource = crate::editor::uri_string(&editor.document.scheme, &editor.document.path);
        self.diagnostics.service.read(ReadOptions { resource: Some(&resource), ..ReadOptions::default() }).iter().any(|marker| {
            let on_line = marker.severity == severity && marker.start_line <= line_number && line_number <= marker.end_line;
            match mode {
                SignalMode::Line => on_line,
                SignalMode::Position => on_line && marker.start_column <= column && column <= marker.end_column,
            }
        })
    }

    /// commands.ts `ShowSignalSoundHelp` and `ShowAccessibilityAnnouncementHelp`: every signal, checked when
    /// enabled; moving plays the sound; accepting writes each signal's setting.
    fn show_signal_help(&mut self, modality: Modality, window: &mut Window, cx: &mut Context<Self>) {
        let field = if modality == Modality::Sound { "sound" } else { "announcement" };
        let mut picks: Vec<(String, &'static str)> = SIGNALS
            .iter()
            .filter(|signal| modality == Modality::Sound || signal.has_legacy_announcement)
            .map(|signal| {
                let label = match USER_GESTURE_SIGNALS.contains(&signal.settings_key) {
                    true => format!("{} ({})", signal.name, self.setting(&format!("{}.{field}", signal.settings_key)).as_str().unwrap_or_default()),
                    false => signal.name.to_owned(),
                };
                (label, signal.settings_key)
            })
            .collect();
        picks.sort_by(|a, b| crate::command_palette::locale_compare(&a.0, &b.0));
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.can_select_many = true;
        let optimized = self.is_screen_reader_optimized();
        input.placeholder = Some(match (modality, optimized) {
            (Modality::Sound, _) => "Select a sound to play and configure".into(),
            (Modality::Announcement, true) => "Select an announcement to configure".into(),
            (Modality::Announcement, false) => "Screen reader is not active, announcements are disabled by default.".into(),
        });
        let items = picks
            .iter()
            .enumerate()
            .map(|(handle, (label, key))| {
                let buttons = match USER_GESTURE_SIGNALS.contains(key) {
                    true => vec![Button { handle: 0, codicon: Some("settings-gear".into()), tooltip: Some(if modality == Modality::Sound { "Configure Sound" } else { "Configure Announcement" }.into()), always_visible: true, ..Button::default() }],
                    false => Vec::new(),
                };
                crate::quick_input::Entry::Item(Item { handle: handle as i64, label: label.clone(), buttons, ..Item::default() })
            })
            .collect();
        input.set_items(items);
        let selected: Vec<i64> = picks
            .iter()
            .enumerate()
            .filter(|(_, (_, key))| {
                let enabled = self.signal_enabled(key, modality) && (modality == Modality::Sound || signal_by_key(key).announcement.is_some());
                enabled || USER_GESTURE_SIGNALS.contains(key) && self.setting(&format!("{key}.{field}")) != json!("never")
            })
            .map(|(handle, _)| handle as i64)
            .collect();
        input.set_selected_items(&selected);
        let keys = picks.into_iter().map(|(_, key)| key).collect();
        let id = self.quick_inputs.add(input, Owner::AccessibilitySignals(SignalHelp { modality, keys }));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn signal_help_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::AccessibilitySignals(help)) = self.quick_inputs.owner(id) else { return };
        let (modality, keys) = (help.modality, help.keys.clone());
        match event {
            Event::ChangeActive(handles) if modality == Modality::Sound => {
                if let Some(&handle) = handles.first() {
                    self.play_signal_sound(signal_by_key(keys[handle as usize]).sounds[0]);
                }
            }
            Event::Accept(_) => {
                let selected: Vec<&str> = self.quick_inputs.get(id).map(|input| input.selected_handles().iter().map(|handle| keys[*handle as usize]).collect()).unwrap_or_default();
                let optimized = self.is_screen_reader_optimized();
                if modality == Modality::Announcement && !optimized {
                    self.hide_quick_input_by_id(id, window, cx);
                    return;
                }
                for key in &keys {
                    let user_gesture = USER_GESTURE_SIGNALS.contains(key);
                    let current = self.setting(key);
                    let value = match (modality, selected.contains(key)) {
                        (Modality::Sound, true) => {
                            let sound = if user_gesture { "userGesture" } else if optimized { "auto" } else { "on" };
                            with_field(&current, "sound", json!(sound), "announcement")
                        }
                        (Modality::Sound, false) => with_field(&current, "sound", json!(disabled_value(user_gesture, optimized)), "announcement"),
                        (Modality::Announcement, true) => {
                            let announcement = if user_gesture { json!("userGesture") } else if signal_by_key(key).announcement.is_some() { json!("auto") } else { Value::Null };
                            json!({ "sound": current["sound"], "announcement": announcement })
                        }
                        (Modality::Announcement, false) => json!({ "sound": current["sound"], "announcement": disabled_value(user_gesture, true) }),
                    };
                    self.update_user_setting(key, without_nulls(value));
                }
                self.hide_quick_input_by_id(id, window, cx);
            }
            Event::TriggerItemButton(handle, ..) => {
                let key = keys[handle as usize];
                let outcome = self.execute_command("workbench.action.openSettingsJson", vec![json!({ "revealSetting": { "key": key, "edit": true } })], window, cx);
                super::quick_access::show_failure("Preferences: Open User Settings (JSON)".into(), outcome, window, cx);
            }
            Event::Hide => {
                self.quick_inputs.remove(id);
            }
            _ => {}
        }
    }

    /// The editor's node for VoiceOver (textAreaEditContext.ts): its aria label, and the page of lines around the
    /// selection as text runs with the selection on them (`SimplePagedScreenReaderStrategy`), computed only while a
    /// screen reader reads the tree.
    pub(super) fn editor_accessibility<E: StatefulInteractiveElement>(&self, element: E, editor: &OpenEditor, window: &Window) -> E {
        let label = match self.setting("editor.accessibilitySupport").as_str() {
            Some("off") => "The editor is not accessible at this time. To enable screen reader optimized mode, use ⇧⌥F1".to_owned(),
            _ => self.setting("editor.ariaLabel").as_str().unwrap_or("Editor content").to_owned(),
        };
        let element = element.role(Role::MultilineTextInput).aria_label(label);
        if !window.is_a11y_active() {
            return element;
        }
        let page_size = self.setting("editor.accessibilityPageSize").as_u64().unwrap_or(500).max(1) as usize;
        let page = screen_reader_page(&editor.document.lines, editor.selections[0].anchor, editor.selections[0].head, page_size);
        element.a11y_synthetic_children(move |builder| page.build(builder))
    }
}

/// `Owner::AccessibilitySignals`: the help picker's modality and the settings key of each item by handle.
pub struct SignalHelp {
    modality: Modality,
    keys: Vec<&'static str>,
}

fn with_field(current: &Value, field: &str, value: Value, other: &str) -> Value {
    json!({ field: value, other: current[other] })
}

fn without_nulls(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, field| !field.is_null());
    }
    value
}

/// commands.ts `getDisabledSettingValue`.
fn disabled_value(user_gesture: bool, screen_reader_optimized: bool) -> &'static str {
    match (user_gesture, screen_reader_optimized) {
        (true, _) => "never",
        (false, true) => "off",
        (false, false) => "auto",
    }
}

/// The lines of the pages around the selection, by 0-based line, and where the selection sits in them (char columns).
pub(crate) struct ScreenReaderPage {
    lines: Vec<(usize, String)>,
    anchor: (usize, usize),
    focus: (usize, usize),
}

/// screenReaderUtils.ts `SimplePagedScreenReaderStrategy`: the page of the selection's start and the page of its
/// end, `page_size` lines each; the pages between them are left out, as upstream joins the two with an ellipsis.
pub(crate) fn screen_reader_page(lines: &[String], anchor: crate::editor::Position, head: crate::editor::Position, page_size: usize) -> ScreenReaderPage {
    let (start, end) = if (anchor.line, anchor.column) <= (head.line, head.column) { (anchor, head) } else { (head, anchor) };
    let page = |line: usize| line / page_size * page_size..((line / page_size + 1) * page_size).min(lines.len());
    let (first, last) = (page(start.line), page(end.line));
    let range: Vec<usize> = if last.start <= first.end { (first.start..last.end).collect() } else { first.chain(last).collect() };
    ScreenReaderPage { lines: range.into_iter().map(|line| (line, lines[line].clone())).collect(), anchor: (anchor.line, anchor.column), focus: (head.line, head.column) }
}

impl ScreenReaderPage {
    pub(super) fn build(self, builder: &mut A11ySubtreeBuilder) {
        let mut runs = Vec::new();
        let mut value = String::new();
        for (index, (line, text)) in self.run_texts().into_iter().enumerate() {
            let mut run = accesskit::Node::new(Role::TextRun);
            run.set_character_lengths(text.chars().map(|char| char.len_utf8() as u8).collect::<Vec<_>>());
            value.push_str(&text);
            run.set_value(text);
            let id = builder.synthetic_node_id(index);
            builder.push_child(id, run);
            if let Some(line) = line {
                runs.push((line, id));
            }
        }
        let position = |(line, column): (usize, usize)| {
            let index = runs.iter().position(|(known, _)| *known == line).unwrap_or(0);
            accesskit::TextPosition { node: runs[index].1, character_index: column.min(self.lines[index].1.chars().count()) }
        };
        let selection = accesskit::TextSelection { anchor: position(self.anchor), focus: position(self.focus) };
        // accesskit_consumer derives AXValue from the runs only for single-line inputs: give the text area the page.
        builder.parent_node().set_value(value);
        builder.parent_node().set_text_selection(selection);
    }

    /// The text area's runs: each line with its newline (the last without), and the `…` upstream puts between the
    /// two pages when pages lie between them.
    fn run_texts(&self) -> Vec<(Option<usize>, String)> {
        let mut runs = Vec::new();
        let last = self.lines.len().saturating_sub(1);
        for (index, (line, text)) in self.lines.iter().enumerate() {
            if index > 0 && self.lines[index - 1].0 + 1 != *line {
                runs.push((None, "\u{2026}".to_owned()));
            }
            runs.push((Some(*line), if index < last { format!("{text}\n") } else { text.clone() }));
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Position;

    fn lines(count: usize) -> Vec<String> {
        (0..count).map(|line| format!("line {line}")).collect()
    }

    #[test]
    fn the_page_holds_the_selection_lines() {
        let page = screen_reader_page(&lines(25), Position { line: 12, column: 2 }, Position { line: 12, column: 2 }, 10);
        assert_eq!((page.lines[0].0, page.lines.len()), (10, 10));
        assert_eq!(page.lines[2].1, "line 12");
        let neighbours = screen_reader_page(&lines(25), Position { line: 3, column: 0 }, Position { line: 12, column: 1 }, 10);
        assert_eq!((neighbours.lines[0].0, neighbours.lines.len()), (0, 20));
        let apart = screen_reader_page(&lines(25), Position { line: 3, column: 0 }, Position { line: 21, column: 1 }, 10);
        let numbers: Vec<usize> = apart.lines.iter().map(|(line, _)| *line).collect();
        assert_eq!(numbers, (0..10).chain(20..25).collect::<Vec<_>>());
        let text: String = apart.run_texts().into_iter().map(|(_, text)| text).collect();
        assert!(text.starts_with("line 0\n") && text.contains("line 9\n\u{2026}line 20\n") && text.ends_with("line 24"));
        let last = screen_reader_page(&lines(25), Position { line: 24, column: 0 }, Position { line: 24, column: 0 }, 10);
        assert_eq!((last.lines[0].0, last.lines.len()), (20, 5));
    }

    #[gpui::test]
    fn errors_are_announced_on_a_new_line_and_at_each_position_on_them(cx: &mut gpui::TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-line-signals-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "let x\nlet y").unwrap();
        let settings = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("user"), None);
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), std::sync::Arc::new(std::sync::Mutex::new(settings)), crate::host_sync::HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench.watch_editor_focus(window, cx);
            let mut settings = workbench.settings.lock().unwrap();
            settings.update_value_derived("editor.accessibilitySupport", Some(&json!("on"))).unwrap();
            for key in ["lineHasError", "positionHasError", "lineHasWarning", "positionHasWarning"] {
                settings.update_value_derived(&format!("accessibility.signals.{key}"), Some(&json!({ "sound": "off", "announcement": "auto" }))).unwrap();
            }
            drop(settings);
            workbench
        });
        cx.update(|window, _| window.activate_window());
        let set_markers = |markers: Vec<Value>, cx: &mut gpui::VisualTestContext| {
            workbench.update(cx, |workbench, cx| {
                let resource = workbench.editors.active().unwrap().document.uri_string();
                workbench.diagnostics.service.change_one("test", &resource, &markers, false);
                cx.notify();
            });
            cx.run_until_parked();
        };
        let marker = |severity: u8, start: usize, end: usize| json!({ "severity": severity, "message": "bad", "startLineNumber": 2, "startColumn": start, "endLineNumber": 2, "endColumn": end });
        set_markers(vec![marker(ERROR, 5, 6)], cx);
        let alert = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.accessibility.alert.clone());

        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        let (message, count) = alert(cx).unwrap();
        assert_eq!(message, "Error on Line");
        cx.simulate_keystrokes("end");
        cx.run_until_parked();
        assert_eq!(alert(cx).unwrap(), ("Error".to_owned(), count + 1));
        cx.simulate_keystrokes("home");
        cx.run_until_parked();
        assert_eq!(alert(cx).unwrap(), ("Error".to_owned(), count + 1), "the line's error already played");
        // A warning appearing at the cursor plays its position signal, which also stands for the line's.
        set_markers(vec![marker(ERROR, 5, 6), marker(WARNING, 1, 2)], cx);
        assert_eq!(alert(cx).unwrap(), ("Warning".to_owned(), count + 2));

        // accessibility.dimUnfocused: the editor dims once the focus leaves it.
        let opacities = workbench.update_in(cx, |workbench, window, cx| {
            workbench.settings.lock().unwrap().update_value_derived("accessibility.dimUnfocused.enabled", Some(&json!(true))).unwrap();
            let focused = workbench.unfocused_editor_opacity(window);
            window.focus(&workbench.status_bar_focus, cx);
            (focused, workbench.unfocused_editor_opacity(window))
        });
        assert_eq!(opacities, (1., 0.75));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn disabled_values_follow_get_disabled_setting_value() {
        assert_eq!(disabled_value(true, false), "never");
        assert_eq!(disabled_value(false, true), "off");
        assert_eq!(disabled_value(false, false), "auto");
    }
}
