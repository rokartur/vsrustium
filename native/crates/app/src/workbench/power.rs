//! MainThreadPower's events (mainThreadPower.ts) and the metered connection of the window: MainThreadMeteredConnection
//! (mainThreadMeteredConnection.ts), the status entry (meteredConnectionStatus.ts) and Configure Metered Connection
//! (meteredConnection.contribution.ts).

use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::main_thread_status_bar::StatusEntry;
use crate::main_thread_quick_open::Owner;
use crate::metered_connection::is_metered;
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

const METERED_STATUS_ID: &str = "workbench.contrib.meteredConnectionStatus";
const METERED_SETTING: &str = "network.meteredConnection";

/// The OS state and what the host was last told.
#[derive(Default)]
pub struct MeteredConnection {
    os_metered: bool,
    sent: Option<bool>,
}

impl Workbench {
    /// Forwards powerMonitor's events to the extension host and watches the network path.
    pub(super) fn start_system_events(&mut self, cx: &mut Context<Self>) {
        let (power_events, mut power) = unbounded();
        crate::power::watch(power_events);
        let (path_updates, mut paths) = unbounded();
        crate::metered_connection::watch(path_updates);
        cx.spawn(async move |workbench, cx| {
            while let Some((method, args)) = power.next().await {
                if workbench.update(cx, |workbench, _| workbench.host.send("ExtHostPower", method, args)).is_err() {
                    return;
                }
            }
        })
        .detach();
        cx.spawn(async move |workbench, cx| {
            while let Some(os_metered) = paths.next().await {
                let updated = workbench.update(cx, |workbench, cx| {
                    workbench.metered.os_metered = os_metered;
                    workbench.update_metered_connection(cx);
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();
        let (display_changes, mut display) = unbounded();
        crate::power::watch_accessibility_display(display_changes);
        cx.spawn(async move |workbench, cx| {
            while display.next().await.is_some() {
                // accessibilityService.ts fires `onDidChangeReducedMotion` for the OS value only while the setting is auto.
                let updated = workbench.update(cx, |workbench, cx| {
                    if workbench.setting("workbench.reduceMotion") == json!("auto") {
                        workbench.restyle_webviews();
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();
        self.commands.register("workbench.action.configureMeteredConnection", |workbench, _, window, cx| {
            workbench.show_metered_connection_picker(window, cx);
            Outcome::done()
        });
    }

    /// `isConnectionMetered` from the setting and the OS; the host hears the first value as `$initialize...` and
    /// changes after, and the status entry shows while metered. Runs on every render, so a settings change applies.
    pub(super) fn update_metered_connection(&mut self, cx: &mut Context<Self>) {
        let setting = self.setting(METERED_SETTING);
        let metered = is_metered(setting.as_str(), self.metered.os_metered);
        if self.metered.sent == Some(metered) {
            return;
        }
        let method = if self.metered.sent.is_none() { "$initializeIsConnectionMetered" } else { "$onDidChangeIsConnectionMetered" };
        self.host.send("ExtHostMeteredConnection", method, vec![json!(metered)]);
        self.metered.sent = Some(metered);
        self.status_entries.retain(|entry| entry.entry_id != METERED_STATUS_ID);
        if metered {
            self.status_entries.push(StatusEntry {
                entry_id: METERED_STATUS_ID.into(),
                id: METERED_STATUS_ID.into(),
                name: "Metered Connection".into(),
                text: "$(radio-tower)".into(),
                tooltip: Some("Metered connection enabled. Some background network activity, including updates, Settings Sync, inline completions, telemetry, and automatic extension updates will be paused or reduced.".into()),
                command: Some(json!({ "id": "workbench.action.configureMeteredConnection", "title": "Configure" })),
                color: None,
                background_color: None,
                kind: Some("warning"),
                align_left: false,
                // -Number.MAX_VALUE: the far right.
                priority: -f64::MAX,
                secondary: 0,
                show_progress: false,
            });
        }
        cx.notify();
    }

    /// `ConfigureMeteredConnectionAction`: Auto, On, Off, the current one active; picking writes the setting.
    fn show_metered_connection_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.setting(METERED_SETTING);
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select Metered Connection Mode".into());
        let items = METERED_PICKS.iter().enumerate().map(|(handle, (_, label, description))| Entry::Item(Item { handle: handle as i64, label: (*label).into(), description: Some((*description).into()), ..Item::default() })).collect();
        input.set_items(items);
        if let Some(active) = METERED_PICKS.iter().position(|(value, ..)| current == json!(value)) {
            input.set_active_items(&[active as i64]);
        }
        let id = self.quick_inputs.add(input, Owner::MeteredConnection);
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn metered_connection_picker_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Accept(_) => {
                let Some(&handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()).as_ref() else { return };
                self.hide_quick_input_by_id(id, window, cx);
                // `configurationService.updateValue(key, value)` without a target.
                let mut settings = self.settings.lock().unwrap();
                match settings.update_value_derived(METERED_SETTING, Some(&Value::from(METERED_PICKS[handle as usize].0))) {
                    Ok(change) if !change.is_empty() => self.host.send_configuration_change(&settings, &change),
                    Ok(_) => {}
                    Err(error) => eprintln!("writing {METERED_SETTING}: {error}"),
                }
                drop(settings);
                self.update_metered_connection(cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

const METERED_PICKS: [(&str, &str, &str); 3] = [
    ("auto", "Auto", "Detect metered connections automatically"),
    ("on", "On", "Always treat the connection as metered"),
    ("off", "Off", "Never treat the connection as metered"),
];
