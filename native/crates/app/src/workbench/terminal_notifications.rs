//! terminal.notification.contribution.ts: a terminal's OSC 99 notifications through the notification service, with
//! its buttons, Dismiss and Disable Terminal Notifications, the `w=` auto close and the reports back to the process.

use std::time::Duration;

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::notifications::{Action, Notification, Run, Severity};
use crate::terminal::osc99::{Effect, Env, Shown};

const ENABLE_NOTIFICATIONS: &str = "terminal.integrated.enableNotifications";
/// Reply values of the secondary actions, past any button index.
const DISMISS: u64 = u64::MAX - 1;
const DISABLE: u64 = u64::MAX;

impl Workbench {
    /// The OSC 99 sequences the instance's last output carried.
    pub(super) fn terminal_osc99(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let group_visible = self.terminal_view.group_instances().into_iter().any(|group| group.contains(&id) && self.terminal_view.active_instance_id().is_some_and(|active| group.contains(&active)));
        let env = Env { enabled: self.setting(ENABLE_NOTIFICATIONS) == true, window_focused: window.is_window_active(), terminal_visible: self.terminal_view_visible() && group_visible };
        let Some(instance) = self.terminal_view.instance_mut(id) else { return };
        let mut effects = Vec::new();
        for data in std::mem::take(&mut instance.osc99_sequences) {
            effects.extend(instance.osc99.handle(&data, &env));
        }
        self.apply_osc99_effects(id, effects, window, cx);
    }

    fn apply_osc99_effects(&mut self, id: u32, effects: Vec<Effect>, window: &mut Window, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Show(shown) => self.show_osc99_notification(id, shown, window, cx),
                Effect::Update(shown) => {
                    // `updateMessage`, `updateSeverity`, `updateActions`: the toast is replaced in place.
                    if let Some(key) = self.osc99_notification_key(id, shown.key) {
                        self.terminal_view.osc99_notifications.retain(|(_, _, notification)| *notification != key);
                        self.notifications.replies.remove(&key);
                        self.close_notification(key, window, cx);
                    }
                    self.show_osc99_notification(id, shown, window, cx);
                }
                Effect::Close(key) => {
                    if let Some(notification) = self.osc99_notification_key(id, key) {
                        self.close_notification(notification, window, cx);
                    }
                }
                Effect::Focus => {
                    self.terminal_view.set_active(id);
                    self.execute_command("workbench.action.terminal.focus", Vec::new(), window, cx);
                }
                Effect::Write(text) => {
                    if let Some(instance) = self.terminal_view.instance_mut(id) {
                        instance.input(&text);
                    }
                }
            }
        }
    }

    fn show_osc99_notification(&mut self, id: u32, shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        let mut notification = Notification::new(if shown.warning { Severity::Warning } else { Severity::Info }, shown.message);
        notification.id = shown.id.as_ref().map(|id| format!("terminal.osc99.{id}"));
        notification.source = Some("Terminal".into());
        notification.source_id = Some("terminal".into());
        if let Some(priority) = shown.priority {
            notification.priority = priority;
        }
        for (index, label) in shown.buttons.iter().enumerate().filter(|(_, label)| !label.is_empty()) {
            notification.primary.push(Action { id: format!("terminal.osc99.button.{index}"), label: label.clone(), run: Run::Reply(index as u64 + 1) });
        }
        notification.secondary = vec![
            Action { id: "terminal.osc99.dismiss".into(), label: "Dismiss".into(), run: Run::Reply(DISMISS) },
            Action { id: "terminal.osc99.disable".into(), label: "Disable Terminal Notifications".into(), run: Run::Reply(DISABLE) },
        ];
        let Some(key) = self.notify(notification) else { return };
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.notifications.replies.insert(key, sender);
        self.terminal_view.osc99_notifications.push((id, shown.key, key));
        let handler_key = shown.key;
        cx.spawn_in(window, async move |workbench, cx| {
            let answer = receiver.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                // Replaced by an update: the new notification answers from now on.
                if !workbench.terminal_view.osc99_notifications.contains(&(id, handler_key, key)) {
                    return;
                }
                let Some(instance) = workbench.terminal_view.instance_mut(id) else { return };
                let effects = match answer {
                    Ok(Ok(Value::Number(reply))) => match reply.as_u64() {
                        Some(DISABLE) => {
                            workbench.update_setting(ENABLE_NOTIFICATIONS, json!(false));
                            let Some(instance) = workbench.terminal_view.instance_mut(id) else { return };
                            instance.osc99.dismissed(handler_key)
                        }
                        Some(DISMISS) => instance.osc99.dismissed(handler_key),
                        Some(button) => instance.osc99.button(handler_key, button as usize),
                        None => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                // A button closes its notification itself; closing it again does nothing.
                workbench.apply_osc99_effects(id, effects, window, cx);
                workbench.terminal_view.osc99_notifications.retain(|entry| *entry != (id, handler_key, key));
                let Some(instance) = workbench.terminal_view.instance_mut(id) else { return };
                let closed = instance.osc99.closed(handler_key);
                workbench.apply_osc99_effects(id, closed, window, cx);
            });
        })
        .detach();
        if let Some(ms) = shown.auto_close_ms {
            cx.spawn_in(window, async move |workbench, cx| {
                cx.background_executor().timer(Duration::from_millis(ms)).await;
                let _ = workbench.update_in(cx, |workbench, window, cx| {
                    let Some(instance) = workbench.terminal_view.instance_mut(id) else { return };
                    let effects = instance.osc99.auto_closed(handler_key);
                    workbench.apply_osc99_effects(id, effects, window, cx);
                });
            })
            .detach();
        }
    }

    fn osc99_notification_key(&self, id: u32, handler_key: u64) -> Option<u64> {
        self.terminal_view.osc99_notifications.iter().find(|(instance, key, _)| *instance == id && *key == handler_key).map(|(_, _, notification)| *notification)
    }
}
