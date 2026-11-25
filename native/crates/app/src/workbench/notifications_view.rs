//! The notification toasts, the notifications center and the status bar bell: browser/parts/notifications
//! notificationsToasts.ts, notificationsCenter.ts, notificationsStatus.ts, notificationsViewer.ts and the commands
//! of notificationsCommands.ts, over the model of notifications.rs.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui::{
    AnyElement, Bounds, BoxShadow, ClickEvent, Context, Div, FocusHandle, FontWeight, InteractiveText, MouseButton, MouseDownEvent, Pixels, Point, Rgba, SharedString,
    Size, Stateful, StyledText, Task, TextRun, Window, div, point, prelude::*, px, rgba,
};
use serde_json::{Value, json};

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use crate::commands::Outcome;
use crate::main_thread_progress::ProgressTask;
use crate::menus::MenuAction;
use crate::notifications::{Action, Item, Node, Notification, Notifications, Priority, Run, Severity};

#[cfg(test)]
mod tests;

/// `NotificationsToasts.MAX_WIDTH`; the center has the same width.
const WIDTH: f32 = 450.;
/// `COMPACT_NOTIFICATION_ROW_HEIGHT`, the row height the workbench sets.
const ROW_HEIGHT: f32 = 34.;
const LINE_HEIGHT: f32 = 22.;
const MAX_TOASTS: usize = 3;
/// `SPAM_PROTECTION`: at most 3 toasts per 800ms.
const SPAM_INTERVAL: Duration = Duration::from_millis(800);
const SPAM_LIMIT: usize = 3;
/// The 1.147 floating layout: the toasts end 4px above the status bar, the center 8px.
const TOASTS_OFFSET: f32 = 4.;
const CENTER_OFFSET: f32 = 8.;
/// window.ts `DEFAULT_CUSTOM_TITLEBAR_HEIGHT`, which the top right position adds.
const TITLE_BAR_HEIGHT: f32 = 35.;
const CENTER_HEADER_HEIGHT: f32 = 35.;
/// The status bar's 28px, its 24px items and 2px padding.
const STATUS_BAR_HEIGHT: f32 = 28.;
/// `NotificationsCenter.MAX_DIMENSIONS` height.
const CENTER_MAX_HEIGHT: f32 = 400.;
const GLOBAL_FILTER_KEY: &str = "notifications.doNotDisturbMode";
const SOURCE_FILTER_KEY: &str = "notifications.perSourceDoNotDisturbMode";

/// The notifications part's state: the model, which items show as toasts, the center, and the pending
/// `$showMessage` answers by notification key.
pub(crate) struct NotificationsUi {
    pub model: Notifications,
    pub center_visible: bool,
    /// Newest first; a toast past `MAX_TOASTS` waits hidden until one above it goes.
    toasts: Vec<Toast>,
    /// Added since the last render, newest last; the render turns them into toasts.
    new_toasts: Vec<u64>,
    recent_toasts: VecDeque<Instant>,
    /// `newNotificationsCount` of notificationsStatus.ts.
    unread: usize,
    /// `INotificationSourceFilter`s in the order their sources first notified: id, label, filtered to errors.
    sources: Vec<(String, String, bool)>,
    pub(crate) replies: HashMap<u64, oneshot::Sender<Result<Value, Value>>>,
    /// The focused row of the toasts or the center list.
    focused: Option<u64>,
    hovered: Option<u64>,
    pub(super) focus: FocusHandle,
    purge: Option<Task<()>>,
    /// The open dropdown's items, command and arguments, run by the `_notifications.menuItem.<index>` commands.
    menu_items: Vec<(String, Vec<Value>)>,
    menu_slots: usize,
    /// The running `$startProgress` tasks by handle.
    pub(crate) progress: HashMap<u64, ProgressTask>,
}

struct Toast {
    key: u64,
    /// When the purge timeout ends; `None` while the window is in the background, which restarts it on return.
    deadline: Option<Instant>,
}

impl NotificationsUi {
    pub fn new(cx: &mut Context<Workbench>) -> Self {
        NotificationsUi {
            model: Notifications::default(),
            center_visible: false,
            toasts: Vec::new(),
            new_toasts: Vec::new(),
            recent_toasts: VecDeque::new(),
            unread: 0,
            sources: Vec::new(),
            replies: HashMap::new(),
            focused: None,
            hovered: None,
            focus: cx.focus_handle(),
            purge: None,
            menu_items: Vec::new(),
            menu_slots: 0,
            progress: HashMap::new(),
        }
    }

    pub fn toasts_visible(&self) -> bool {
        !self.toasts.is_empty()
    }

    pub fn toast_shown(&self, key: u64) -> bool {
        self.toasts.iter().take(MAX_TOASTS).any(|toast| toast.key == key)
    }
}

impl Workbench {
    pub(super) fn register_notifications_commands(&mut self) {
        self.commands.register("notifications.showList", |workbench, _, window, cx| {
            workbench.show_notifications_center(window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.hideList", |workbench, _, window, cx| {
            workbench.hide_notifications_center(window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.toggleList", |workbench, _, window, cx| {
            match workbench.notifications.center_visible {
                true => workbench.hide_notifications_center(window, cx),
                false => workbench.show_notifications_center(window, cx),
            }
            Outcome::done()
        });
        self.commands.register("notifications.clearAll", |workbench, _, window, cx| {
            workbench.clear_all_notifications(window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.hideToasts", |workbench, _, window, cx| {
            workbench.hide_toasts(window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.focusToasts", |workbench, _, window, cx| {
            workbench.focus_toast(|toasts, _| toasts.first().copied(), window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.focusFirstToast", |workbench, _, window, cx| {
            workbench.focus_toast(|toasts, _| toasts.first().copied(), window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.focusLastToast", |workbench, _, window, cx| {
            workbench.focus_toast(|toasts, _| toasts.last().copied(), window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.focusNextToast", |workbench, _, window, cx| {
            workbench.focus_toast(|toasts, index| index.and_then(|index| toasts.get(index + 1)).or(toasts.first()).copied(), window, cx);
            Outcome::done()
        });
        self.commands.register("notifications.focusPreviousToast", |workbench, _, window, cx| {
            workbench.focus_toast(|toasts, index| index.and_then(|index| index.checked_sub(1)).and_then(|index| toasts.get(index)).or(toasts.last()).copied(), window, cx);
            Outcome::done()
        });
        self.commands.register("notification.clear", |workbench, args, window, cx| {
            if let Some(key) = workbench.notification_from_context(&args).filter(|key| !workbench.notifications.model.item(*key).is_some_and(Item::has_active_progress)) {
                workbench.close_notification(key, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("notification.expand", |workbench, args, _, cx| {
            workbench.change_notification(&args, Item::expand, cx);
            Outcome::done()
        });
        self.commands.register("notification.collapse", |workbench, args, _, cx| {
            workbench.change_notification(&args, Item::collapse, cx);
            Outcome::done()
        });
        self.commands.register("notification.toggle", |workbench, args, _, cx| {
            workbench.change_notification(&args, Item::toggle, cx);
            Outcome::done()
        });
        self.commands.register("notification.acceptPrimaryAction", |workbench, args, window, cx| {
            let key = workbench.notification_from_context(&args).or(workbench.notifications.model.items.first().map(|item| item.key));
            if let Some(key) = key.filter(|key| workbench.notifications.model.item(*key).is_some_and(|item| !item.primary.is_empty())) {
                workbench.run_notification_action(key, 0, window, cx);
            }
            Outcome::done()
        });
        for (id, value) in [("bottomRight", "bottom-right"), ("bottomLeft", "bottom-left"), ("topRight", "top-right")] {
            self.commands.register(format!("workbench.action.setNotificationsPosition.{id}"), move |workbench, _, _, cx| {
                workbench.update_setting("workbench.notifications.position", json!(value));
                cx.notify();
                Outcome::done()
            });
        }
        self.commands.register("notifications.toggleDoNotDisturbMode", |workbench, _, window, cx| {
            let enabled = !workbench.notifications.model.do_not_disturb;
            workbench.set_do_not_disturb(enabled, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.copyNotificationMessage", |workbench, args, _, cx| {
            if let Some(item) = workbench.notification_from_context(&args).and_then(|key| workbench.notifications.model.item(key)) {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(item.message.raw.clone()));
            }
            Outcome::done()
        });
        self.commands.register("_notifications.setSourceFilter", |workbench, args, _, cx| {
            if let (Some(id), Some(muted)) = (args.first().and_then(Value::as_str), args.get(1).and_then(Value::as_bool)) {
                workbench.set_source_filter(id, muted);
                cx.notify();
            }
            Outcome::done()
        });
        self.commands.register("_notifications.runSecondaryAction", |workbench, args, window, cx| {
            let (Some(key), Some(index)) = (args.first().and_then(Value::as_u64), args.get(1).and_then(Value::as_u64)) else { return Outcome::done() };
            let Some(action) = workbench.notifications.model.item(key).and_then(|item| item.secondary.get(index as usize)).cloned() else { return Outcome::done() };
            workbench.run_action(key, action, window, cx)
        });
    }

    /// `INotificationService.notify`: the notification joins the center and, unless silent, shows as a toast.
    pub(crate) fn notify(&mut self, notification: Notification) -> Option<u64> {
        let source = notification.source.clone().zip(notification.source_id.clone());
        let ui = &mut self.notifications;
        if let Some((label, id)) = source {
            match ui.sources.iter_mut().find(|(source, ..)| *source == id) {
                Some(existing) => existing.1 = label,
                None => ui.sources.push((id, label, false)),
            }
        }
        let added = ui.model.add(notification)?;
        if let Some(replaced) = added.replaced {
            ui.toasts.retain(|toast| toast.key != replaced.key);
            if let Some(reply) = ui.replies.remove(&replaced.key) {
                let _ = reply.send(Ok(Value::Null));
            }
        } else if !ui.center_visible {
            ui.unread += 1;
        }
        let item = ui.model.item(added.key).expect("just added");
        let now = Instant::now();
        while ui.recent_toasts.front().is_some_and(|added| now.duration_since(*added) > SPAM_INTERVAL) {
            ui.recent_toasts.pop_front();
        }
        if !ui.center_visible && item.priority != Priority::Silent {
            ui.recent_toasts.push_back(now);
            if ui.recent_toasts.len() <= SPAM_LIMIT {
                ui.new_toasts.push(added.key);
            }
        }
        Some(added.key)
    }

    pub(crate) fn notify_error(&mut self, message: impl Into<String>) {
        self.notify(Notification::new(Severity::Error, message));
    }

    pub(crate) fn notify_warning(&mut self, message: impl Into<String>) {
        self.notify(Notification::new(Severity::Warning, message));
    }

    /// `INotificationService.prompt`: answers the index of the picked choice, or null when the notification closes.
    pub(crate) fn prompt_notification(&mut self, severity: Severity, message: impl Into<String>, choices: &[&str]) -> oneshot::Receiver<Result<Value, Value>> {
        let mut notification = Notification::new(severity, message);
        notification.primary = choices.iter().enumerate().map(|(index, label)| Action { id: format!("workbench.notification.choice.{index}"), label: (*label).into(), run: Run::Reply(index as u64) }).collect();
        let (sender, receiver) = oneshot::channel();
        if let Some(key) = self.notify(notification) {
            self.notifications.replies.insert(key, sender);
        }
        receiver
    }

    /// `NotificationViewItem.close`: the item leaves the center and its toast; a pending `$showMessage` answers nothing.
    pub(crate) fn close_notification(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.notifications;
        if ui.model.close(key).is_none() {
            return;
        }
        if let Some(reply) = ui.replies.remove(&key) {
            let _ = reply.send(Ok(Value::Null));
        }
        if !ui.center_visible && ui.unread > 0 {
            ui.unread -= 1;
        }
        let had_focus = ui.focus.is_focused(window);
        ui.new_toasts.retain(|toast| *toast != key);
        if let Some(index) = ui.toasts.iter().position(|toast| toast.key == key) {
            ui.toasts.remove(index);
            if ui.focused == Some(key) {
                ui.focused = ui.toasts.get(index).or(ui.toasts.get(index.wrapping_sub(1))).map(|toast| toast.key);
            }
        }
        if ui.center_visible {
            if ui.focused == Some(key) {
                ui.focused = ui.model.items.first().map(|item| item.key);
            }
            if ui.model.items.is_empty() {
                ui.center_visible = false;
            }
        }
        let center_or_toasts = ui.center_visible || !ui.toasts.is_empty();
        if had_focus && !center_or_toasts {
            ui.focused = None;
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// Runs a primary action, as its button does, and closes the notification.
    fn run_notification_action(&mut self, key: u64, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.notifications.model.item(key).and_then(|item| item.primary.get(index)).cloned() else { return };
        drop(self.run_action(key, action, window, cx));
        self.close_notification(key, window, cx);
    }

    fn run_action(&mut self, key: u64, action: Action, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        match action.run {
            Run::Reply(handle) => {
                if let Some(reply) = self.notifications.replies.remove(&key) {
                    let _ = reply.send(Ok(json!(handle)));
                }
                Outcome::done()
            }
            Run::Command(id, args) => {
                let outcome = self.execute_command(&id, args, window, cx);
                super::keyboard::report_failure(outcome, cx);
                Outcome::done()
            }
        }
    }

    fn show_notifications_center(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_toasts(window, cx);
        let ui = &mut self.notifications;
        ui.center_visible = true;
        ui.unread = 0;
        ui.focused = ui.model.items.first().map(|item| item.key);
        window.focus(&ui.focus, cx);
        cx.notify();
    }

    fn hide_notifications_center(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.notifications;
        if !ui.center_visible {
            return;
        }
        ui.center_visible = false;
        ui.unread = 0;
        if ui.focus.is_focused(window) {
            ui.focused = None;
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    /// `NotificationsCenter.clearAll`: hides the center, then closes every notification without running progress.
    fn clear_all_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_notifications_center(window, cx);
        let keys: Vec<u64> = self.notifications.model.items.iter().filter(|item| !item.has_active_progress()).map(|item| item.key).collect();
        for key in keys {
            self.close_notification(key, window, cx);
        }
    }

    /// `NotificationsToasts.hide`: every toast goes; the notifications stay in the center.
    fn hide_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.notifications;
        let had_focus = ui.focus.is_focused(window) && !ui.center_visible;
        ui.toasts.clear();
        ui.new_toasts.clear();
        if had_focus {
            ui.focused = None;
            window.focus(&self.editor_focus, cx);
        }
        cx.notify();
    }

    fn focus_toast(&mut self, pick: impl Fn(&[u64], Option<usize>) -> Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.notifications;
        let visible: Vec<u64> = ui.toasts.iter().take(MAX_TOASTS).map(|toast| toast.key).collect();
        let index = ui.focused.and_then(|key| visible.iter().position(|toast| *toast == key));
        let Some(key) = pick(&visible, index) else { return };
        ui.focused = Some(key);
        window.focus(&ui.focus, cx);
        cx.notify();
    }

    /// `getNotificationFromContext`: the notification an action passed, else the focused one.
    fn notification_from_context(&self, args: &[Value]) -> Option<u64> {
        args.first().and_then(Value::as_u64).or(self.notifications.focused).filter(|key| self.notifications.model.item(*key).is_some())
    }

    fn change_notification(&mut self, args: &[Value], change: fn(&mut Item) -> bool, cx: &mut Context<Self>) {
        let Some(key) = self.notification_from_context(args) else { return };
        if change(self.notifications.model.item_mut(key).expect("checked in context")) {
            cx.notify();
        }
    }

    /// `setFilter(NotificationsFilter)`: Do Not Disturb hides the center and silences new non-error notifications.
    fn set_do_not_disturb(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.notifications.model.do_not_disturb = enabled;
        self.profile_storage.lock().unwrap().set(GLOBAL_FILTER_KEY, enabled.to_string());
        if enabled {
            self.hide_notifications_center(window, cx);
        }
        cx.notify();
    }

    fn set_source_filter(&mut self, id: &str, muted: bool) {
        let ui = &mut self.notifications;
        let Some(source) = ui.sources.iter_mut().find(|(source, ..)| source == id) else { return };
        source.2 = muted;
        ui.model.muted_sources = ui.sources.iter().filter(|(.., muted)| *muted).map(|(id, ..)| id.clone()).collect();
        let stored: Vec<Value> = ui.sources.iter().map(|(id, label, muted)| json!({ "id": id, "label": label, "filter": if *muted { 1 } else { 0 } })).collect();
        self.profile_storage.lock().unwrap().set(SOURCE_FILTER_KEY, Value::Array(stored).to_string());
    }

    /// NotificationService's constructor: the filters from application storage.
    pub(super) fn load_notification_filters(&mut self) {
        let storage = self.profile_storage.lock().unwrap();
        let ui = &mut self.notifications;
        ui.model.do_not_disturb = storage.get(GLOBAL_FILTER_KEY) == Some("true");
        let stored: Vec<Value> = storage.get(SOURCE_FILTER_KEY).and_then(|value| serde_json::from_str(value).ok()).unwrap_or_default();
        ui.sources = stored
            .iter()
            .filter_map(|source| Some((source["id"].as_str()?.to_owned(), source["label"].as_str()?.to_owned(), source["filter"] == json!(1))))
            .collect();
        ui.model.muted_sources = ui.sources.iter().filter(|(.., muted)| *muted).map(|(id, ..)| id.clone()).collect();
    }

    pub(super) fn notifications_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let ui = &self.notifications;
        context.insert("notificationCenterVisible".into(), json!(ui.center_visible));
        context.insert("notificationToastsVisible".into(), json!(ui.toasts_visible()));
        context.insert("notificationFocus".into(), json!(ui.focus.is_focused(window) && ui.focused.is_some()));
    }

    /// Turns new notifications into toasts and removes the toasts whose timeout ended (`purgeNotification`):
    /// never a sticky one, the focused one, the one under the pointer, or any while the window is in the background.
    fn sync_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let active = window.is_window_active();
        let ui = &mut self.notifications;
        for key in ui.new_toasts.drain(..) {
            ui.toasts.insert(0, Toast { key, deadline: None });
        }
        let focused = ui.focus.is_focused(window).then_some(ui.focused).flatten();
        let mut removed = false;
        let model = &ui.model;
        ui.toasts.retain_mut(|toast| {
            let Some(item) = model.item(toast.key) else { return false };
            let timeout = purge_timeout(item.severity);
            match toast.deadline {
                None if active => toast.deadline = Some(now + timeout),
                Some(_) if !active => toast.deadline = None,
                Some(deadline) if deadline <= now => {
                    if item.sticky() || focused == Some(toast.key) || ui.hovered == Some(toast.key) {
                        toast.deadline = Some(deadline + timeout);
                    } else {
                        removed = true;
                        return false;
                    }
                }
                _ => {}
            }
            true
        });
        if removed && ui.toasts.is_empty() && ui.focus.is_focused(window) && !ui.center_visible {
            ui.focused = None;
            window.focus(&self.editor_focus, cx);
        }
        let next = ui.toasts.iter().filter_map(|toast| toast.deadline).min();
        ui.purge = next.map(|deadline| {
            cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(deadline.saturating_duration_since(Instant::now())).await;
                let _ = workbench.update(cx, |_, cx| cx.notify());
            })
        });
    }

    /// The toasts above the status bar, newest on top, and the center when it shows.
    pub(super) fn notifications_widget(&mut self, viewport: Size<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_toasts(window, cx);
        let position = self.notifications_position();
        if self.notifications.center_visible {
            let center = self.notifications_center(viewport, window, cx);
            return Some(place(center, position, CENTER_OFFSET).into_any_element());
        }
        if self.notifications.toasts.is_empty() {
            return None;
        }
        let keys: Vec<u64> = self.notifications.toasts.iter().take(MAX_TOASTS).map(|toast| toast.key).collect();
        let last = keys.len() - 1;
        let toasts = keys.into_iter().enumerate().filter_map(|(index, key)| {
            let item = self.notifications.model.item(key)?.clone();
            let toast = self.notification_row(&item, false, false, window, cx);
            let hovered_key = key;
            Some(
                div()
                    .id(("notification-toast", key))
                    .mx(px(4.))
                    .mt(px(4.))
                    .when(index == last, |toast| toast.mb(px(4.)))
                    .w(px(WIDTH + 2.))
                    .border_1()
                    .border_color(self.color("notificationToast.border"))
                    .rounded(px(8.))
                    .bg(self.color("notifications.background"))
                    .shadow(vec![shadow()])
                    .overflow_hidden()
                    .on_hover(cx.listener(move |workbench, hovered: &bool, _, cx| {
                        let ui = &mut workbench.notifications;
                        if *hovered {
                            ui.hovered = Some(hovered_key);
                        } else if ui.hovered == Some(hovered_key) {
                            ui.hovered = None;
                        }
                        cx.notify();
                    }))
                    .child(toast),
            )
        }).collect::<Vec<_>>();
        let container = div().flex().flex_col().occlude().track_focus(&self.notifications.focus).children(toasts);
        Some(place(container, position, TOASTS_OFFSET).into_any_element())
    }

    /// `getNotificationsPosition`.
    fn notifications_position(&self) -> Position {
        match self.setting("workbench.notifications.position").as_str() {
            Some("bottom-left") => Position::BottomLeft,
            Some("top-right") => Position::TopRight,
            _ => Position::BottomRight,
        }
    }

    fn notifications_center(&mut self, viewport: Size<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let items = self.notifications.model.items.clone();
        let available = f32::from(viewport.height) - STATUS_BAR_HEIGHT - TITLE_BAR_HEIGHT - 24. - CENTER_HEADER_HEIGHT;
        let top_right = self.notifications_position() == Position::TopRight;
        let list_height: f32 = items.iter().map(|item| self.notification_height(item, window)).sum::<f32>().min(CENTER_MAX_HEIGHT.min(available));
        let title = if items.is_empty() { "No new notifications" } else { "Notifications" };
        let dnd = self.notifications.model.do_not_disturb;
        let header = div()
            .h(px(CENTER_HEADER_HEIGHT))
            .pl(px(8.))
            .pr(px(2.))
            .flex()
            .items_center()
            .bg(self.color("notificationCenterHeader.background"))
            .when_some(self.theme.color("notificationCenterHeader.foreground"), |header, color| header.text_color(color))
            .child(div().flex_1().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(self.notification_toolbar_button("notifications-clear-all", "clear-all", "Clear All Notifications", Some("notifications.clearAll"), cx, |workbench, _, window, cx| {
                workbench.clear_all_notifications(window, cx)
            }))
            .child(self.notification_toolbar_button("notifications-dnd", "bell-slash", "Configure Do Not Disturb...", None, cx, move |workbench, event, window, cx| {
                workbench.show_do_not_disturb_menu(event, dnd, window, cx)
            }))
            .child(self.notification_toolbar_button("notifications-position", "arrow-swap", "Configure Notifications Position...", None, cx, |workbench, event, window, cx| {
                workbench.show_notifications_position_menu(event, window, cx)
            }))
            .child(self.notification_toolbar_button("notifications-hide", if top_right { "chevron-up" } else { "chevron-down" }, "Hide Notifications", Some("notifications.hideList"), cx, |workbench, _, window, cx| {
                workbench.hide_notifications_center(window, cx)
            }));
        let focused = self.notifications.focus.is_focused(window).then_some(self.notifications.focused).flatten();
        let rows = items.iter().enumerate().map(|(index, item)| {
            let row = self.notification_row(item, true, focused == Some(item.key), window, cx);
            div().when(index > 0, |row| row.border_t_1().border_color(self.color("notifications.border"))).child(row)
        }).collect::<Vec<_>>();
        div()
            .id("notifications-center")
            .w(px(WIDTH + 2.))
            .border_1()
            .border_color(self.color("notificationCenter.border"))
            .rounded(px(8.))
            .overflow_hidden()
            .shadow(vec![shadow()])
            .bg(self.color("notifications.background"))
            .text_color(self.color("notifications.foreground"))
            .occlude()
            .track_focus(&self.notifications.focus)
            .child(header)
            .child(div().id("notifications-center-list").h(px(list_height)).overflow_y_scroll().children(rows))
    }

    /// `NotificationsListDelegate.getHeight`: a row, plus the message's extra lines and the details row when expanded.
    fn notification_height(&self, item: &Item, window: &Window) -> f32 {
        if !item.expanded {
            return ROW_HEIGHT;
        }
        let actions = usize::from(!item.has_active_progress()) + usize::from(item.can_collapse()) + usize::from(!item.secondary.is_empty());
        let width = WIDTH - (10. + 30. + actions as f32 * 30. - actions.saturating_sub(1) as f32 * 4.);
        let lines = message_lines(&item.message.text(), width, window);
        let mut height = ROW_HEIGHT + (lines - 1) as f32 * LINE_HEIGHT;
        if item.source.is_some() || !item.primary.is_empty() {
            height += ROW_HEIGHT;
        }
        height
    }

    /// `NotificationTemplateRenderer`: the main row (icon, message, toolbar) over the details row (source, buttons).
    fn notification_row(&mut self, item: &Item, in_center: bool, focused: bool, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let key = item.key;
        let height = self.notification_height(item, window);
        // An item whose rows fit one line is not expandable (`collapse(true)` in getHeight).
        let expanded = item.expanded && height > ROW_HEIGHT;
        let hovered = self.notifications.hovered == Some(key) || in_center && self.notifications.focus.is_focused(window) || focused;
        let message_width = WIDTH - 4. - 30.;
        let overflows = !expanded && text_width(&item.message.text(), window) > message_width - toolbar_width(item, true);
        let icon_color = self.color(match item.severity {
            Severity::Info => "notificationsInfoIcon.foreground",
            Severity::Warning => "notificationsWarningIcon.foreground",
            Severity::Error => "notificationsErrorIcon.foreground",
        });
        let toolbar = (expanded || hovered || !in_center && self.notifications.focused == Some(key)).then(|| self.notification_item_toolbar(item, expanded, overflows, cx));
        let main_row = div()
            .flex_1()
            .flex()
            .child(self.icon(item.severity.codicon(), 16., 16., icon_color).h(px(LINE_HEIGHT)).ml(px(6.)).mr(px(8.)))
            .child(self.notification_message(item, expanded, cx))
            .children(toolbar);
        let details = (expanded && (item.source.is_some() || !item.primary.is_empty())).then(|| {
            let buttons = item.primary.iter().enumerate().map(|(index, action)| self.notification_button(key, index, action, cx));
            div()
                .h(px(26.))
                .pl(px(5.))
                .flex()
                .items_center()
                .overflow_hidden()
                .child(
                    div()
                        .flex_1()
                        .ml(px(24.))
                        .text_size(px(12.))
                        .text_color(self.color("descriptionForeground"))
                        .truncate()
                        .children(item.source.as_ref().map(|source| format!("Source: {source}"))),
                )
                .child(div().flex().children(buttons))
        });
        let focus_border = self.color("focusBorder");
        div()
            .id(("notification", key))
            .h(px(height))
            .px(px(2.))
            .py(px(6.))
            .flex()
            .flex_col()
            .rounded(px(4.))
            .border_1()
            .border_color(if focused { focus_border } else { rgba(0).into() })
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                workbench.notifications.focused = Some(key);
                window.focus(&workbench.notifications.focus, cx);
                cx.notify();
            }))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, _, cx| {
                // notificationsList.ts: a double click toggles the expansion.
                if event.click_count() == 2 && workbench.notifications.model.item_mut(key).is_some_and(Item::toggle) {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Middle, cx.listener(move |workbench, _, window, cx| {
                // notificationsViewer.ts: a middle click closes a notification without running progress.
                if !workbench.notifications.model.item(key).is_some_and(Item::has_active_progress) {
                    workbench.close_notification(key, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                let copy = MenuAction::Command { id: "workbench.action.copyNotificationMessage".into(), title: "Copy Text".into(), enabled: true, checked: false, alt: None };
                let context = workbench.key_context(window);
                workbench.show_context_menu_actions(vec![copy], &context, Bounds { origin: event.position, size: Default::default() }, vec![json!(key)], window, cx);
            }))
            .relative()
            .child(main_row)
            .children(details)
            .children(self.notification_progress(item))
    }

    /// notificationsViewer.ts `renderProgress`: a 2px bar at the row's bottom, infinite or worked of total.
    fn notification_progress(&self, item: &Item) -> Option<Div> {
        let progress = item.progress.as_ref()?;
        let color = self.color("progressBar.background");
        let bar = div().absolute().left_0().bottom_0().w_full().h(px(2.)).overflow_hidden();
        Some(match (progress.infinite, progress.total, progress.worked) {
            (false, Some(total), Some(worked)) if total > 0. => bar.child(div().h_full().w(gpui::relative((worked / total).min(1.) as f32)).bg(color)),
            (false, _, _) => bar,
            (true, _, _) => bar.child(super::quick_input_view::infinite_progress_bit("notification-progress", color)),
        })
    }

    /// The message, one line with an ellipsis when collapsed, wrapped when expanded; links run their target.
    fn notification_message(&self, item: &Item, expanded: bool, cx: &mut Context<Self>) -> Div {
        let mut text = String::new();
        let mut links = Vec::new();
        let mut hrefs = Vec::new();
        for node in &item.message.nodes {
            match node {
                Node::Text(value) => text.push_str(value),
                Node::Link { label, href, .. } => {
                    links.push(text.len()..text.len() + label.len());
                    hrefs.push(href.clone());
                    text.push_str(label);
                }
            }
        }
        let link_color = self.color("notificationLink.foreground");
        let highlights = links.iter().map(|range| (range.clone(), gpui::HighlightStyle { color: Some(link_color.into()), ..Default::default() })).collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        let message = InteractiveText::new(("notification-message", item.key), StyledText::new(text).with_highlights(highlights)).on_click(links, move |index, window, cx| {
            let href = hrefs[index].clone();
            let _ = weak.update(cx, |workbench, cx| workbench.open_notification_link(&href, window, cx));
        });
        div()
            .flex_1()
            .min_w_0()
            .line_height(px(LINE_HEIGHT))
            .text_size(px(13.))
            .overflow_hidden()
            .when(!expanded, |message| message.whitespace_nowrap().text_ellipsis())
            .child(message)
    }

    /// The opener service for a message link: a `command:` link runs the command with its JSON arguments.
    pub(super) fn open_notification_link(&mut self, href: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = href.strip_prefix("command:") {
            let (id, query) = command.split_once('?').unwrap_or((command, ""));
            let decoded = percent_decode(query);
            let args = match serde_json::from_str::<Value>(&decoded) {
                Ok(Value::Array(args)) => args,
                Ok(arg) => vec![arg],
                Err(_) => Vec::new(),
            };
            let outcome = self.execute_command(id, args, window, cx);
            super::keyboard::report_failure(outcome, cx);
        } else if let Some(path) = href.strip_prefix("file://") {
            self.open_file(percent_decode(path).into(), true, window, cx);
        } else {
            cx.open_url(href);
        }
    }

    /// `renderSecondaryActions`: More Actions when there are secondary actions, expand or collapse, and close.
    fn notification_item_toolbar(&self, item: &Item, expanded: bool, overflows: bool, cx: &mut Context<Self>) -> Div {
        let key = item.key;
        let mut toolbar = div().flex().h(px(LINE_HEIGHT));
        if !item.secondary.is_empty() || item.source_id.is_some() {
            toolbar = toolbar.child(self.notification_toolbar_button(("notification-configure", key), "gear", "More Actions...", None, cx, move |workbench, event, window, cx| {
                workbench.show_notification_actions_menu(key, event, window, cx)
            }));
        }
        let can_toggle = item.can_collapse() && (expanded || item.source.is_some() || overflows);
        if can_toggle {
            // notificationsActions.ts: the top right position flips the chevrons.
            let top_right = self.notifications_position() == Position::TopRight;
            let (icon, title, command) = match expanded {
                true => (if top_right { "chevron-up" } else { "chevron-down" }, "Collapse Notification", "notification.collapse"),
                false => (if top_right { "chevron-down" } else { "chevron-up" }, "Expand Notification", "notification.expand"),
            };
            toolbar = toolbar.child(self.notification_toolbar_button(("notification-toggle", key), icon, title, Some(command), cx, move |workbench, _, _, cx| {
                if workbench.notifications.model.item_mut(key).is_some_and(Item::toggle) {
                    cx.notify();
                }
            }));
        }
        if !item.has_active_progress() {
            toolbar = toolbar.child(self.notification_toolbar_button(("notification-clear", key), "close", "Clear Notification", Some("notification.clear"), cx, move |workbench, _, window, cx| {
                workbench.close_notification(key, window, cx)
            }));
        }
        toolbar
    }

    /// An `action-label` codicon: 22px, 3px padding, 4px radius, `toolbar.hoverBackground` on hover.
    fn notification_toolbar_button(
        &self,
        id: impl Into<gpui::ElementId>,
        icon: &str,
        title: &'static str,
        command: Option<&'static str>,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Workbench, &ClickEvent, &mut Window, &mut Context<Workbench>) + 'static,
    ) -> Stateful<Div> {
        let hover = match command {
            Some(command) => HoverContent::Action { title, command },
            None => HoverContent::Text(title.into()),
        };
        let id = id.into();
        self.icon(icon, 22., 16., self.color("icon.foreground"))
            .id(id.clone())
            .mr(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                cx.stop_propagation();
                run(workbench, event, window, cx)
            }))
            .child(self.hover_target(id, hover, HoverOptions::mouse()))
    }

    /// A `monaco-text-button` of the buttons row: the first is primary, the rest secondary.
    fn notification_button(&self, key: u64, index: usize, action: &Action, cx: &mut Context<Self>) -> Stateful<Div> {
        let primary = index == 0;
        let (background, foreground, hover) = match primary {
            true => ("button.background", "button.foreground", "button.hoverBackground"),
            false => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
        };
        let hover_color = self.color(hover);
        div()
            .id(("notification-button", key * 64 + index as u64))
            .h(px(26.))
            .px(px(8.))
            .mr(px(4.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .text_size(px(12.))
            .line_height(px(16.))
            .cursor_pointer()
            .bg(self.color(background))
            .text_color(self.color(foreground))
            .when_some(self.theme.color(if primary { "button.border" } else { "button.secondaryBorder" }), |button, border| button.border_1().border_color(border))
            .hover(move |button| button.bg(hover_color))
            .whitespace_nowrap()
            .child(action.label.clone())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                workbench.run_notification_action(key, index, window, cx);
            }))
    }

    /// ConfigureNotificationAction's dropdown: the source's filter toggle, then the secondary actions.
    fn show_notification_actions_menu(&mut self, key: u64, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.notifications.model.item(key) else { return };
        let mut items = Vec::new();
        if let (Some(id), Some(label)) = (&item.source_id, &item.source) {
            let muted = self.notifications.model.muted_sources.contains(id);
            let title = match muted {
                true => format!("Turn On All Notifications from '{label}'"),
                false => format!("Turn Off Info and Warning Notifications from '{label}'"),
            };
            items.push(Some((title, true, false, "_notifications.setSourceFilter".to_owned(), vec![json!(id), json!(!muted)])));
            if !item.secondary.is_empty() {
                items.push(None);
            }
        }
        for (index, action) in item.secondary.iter().enumerate() {
            items.push(Some((action.label.clone(), true, false, "_notifications.runSecondaryAction".to_owned(), vec![json!(key), json!(index)])));
        }
        self.show_notifications_menu(items, event.position(), window, cx);
    }

    /// ConfigureNotificationsPositionAction's dropdown: `MenuId.NotificationsCenterPositionMenu`.
    fn show_notifications_position_menu(&mut self, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.notifications_position();
        let items = [(Position::BottomRight, "Bottom Right", "bottomRight"), (Position::BottomLeft, "Bottom Left", "bottomLeft"), (Position::TopRight, "Top Right", "topRight")]
            .into_iter()
            .map(|(each, title, id)| Some((title.to_owned(), true, each == position, format!("workbench.action.setNotificationsPosition.{id}"), Vec::new())))
            .collect();
        self.show_notifications_menu(items, event.position(), window, cx);
    }

    /// The center's Do Not Disturb dropdown: the global toggle, then each source's checked state.
    fn show_do_not_disturb_menu(&mut self, event: &ClickEvent, dnd: bool, window: &mut Window, cx: &mut Context<Self>) {
        let title = if dnd { "Disable Do Not Disturb Mode" } else { "Enable Do Not Disturb Mode" };
        let mut items = vec![Some((title.to_owned(), true, false, "notifications.toggleDoNotDisturbMode".to_owned(), Vec::new()))];
        let mut sources = self.notifications.sources.clone();
        sources.sort_by(|a, b| a.1.cmp(&b.1));
        if !sources.is_empty() {
            items.push(None);
        }
        for (id, label, muted) in sources {
            items.push(Some((label, !dnd, !muted, "_notifications.setSourceFilter".to_owned(), vec![json!(id), json!(!muted)])));
        }
        self.show_notifications_menu(items, event.position(), window, cx);
    }

    /// A dropdown of title, enabled, checked, command and arguments rows; `None` is a separator.
    fn show_notifications_menu(&mut self, items: Vec<Option<(String, bool, bool, String, Vec<Value>)>>, origin: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.notifications;
        ui.menu_items.clear();
        let mut actions = Vec::new();
        for item in items {
            let Some((title, enabled, checked, command, args)) = item else {
                actions.push(MenuAction::Separator);
                continue;
            };
            actions.push(MenuAction::Command { id: format!("_notifications.menuItem.{}", ui.menu_items.len()), title, enabled, checked, alt: None });
            ui.menu_items.push((command, args));
        }
        while self.notifications.menu_slots < self.notifications.menu_items.len() {
            let slot = self.notifications.menu_slots;
            self.commands.register(format!("_notifications.menuItem.{slot}"), move |workbench, _, window, cx| {
                let Some((command, args)) = workbench.notifications.menu_items.get(slot).cloned() else { return Outcome::done() };
                workbench.execute_command(&command, args, window, cx)
            });
            self.notifications.menu_slots += 1;
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, Bounds { origin, size: Default::default() }, Vec::new(), window, cx);
    }

    /// The `status.notifications` entry of notificationsStatus.ts on the status bar side `left` names: right for the
    /// bottom right position, left for bottom left, and none for top right.
    pub(super) fn notifications_status_item(&self, left: bool, foreground: Rgba, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let position = self.notifications_position();
        if position == Position::TopRight || left != (position == Position::BottomLeft) {
            return None;
        }
        let ui = &self.notifications;
        let in_progress = match ui.center_visible || ui.toasts_visible() {
            true => 0,
            false => ui.model.items.iter().filter(|item| item.has_active_progress()).count(),
        };
        let dot = in_progress > 0 || ui.unread > 0;
        let (icon, tooltip) = match (ui.model.do_not_disturb, dot) {
            (true, true) => ("bell-slash-dot", "Do Not Disturb Mode is Enabled".to_owned()),
            (true, false) => ("bell-slash", "Do Not Disturb Mode is Enabled".to_owned()),
            (false, dot) => (if dot { "bell-dot" } else { "bell" }, self.notifications_tooltip(in_progress)),
        };
        let item = self
            .icon(icon, 28., 16., foreground)
            .id("status.notifications")
            .h_full()
            .cursor_pointer()
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| {
                let command = if workbench.notifications.center_visible { "notifications.hideList" } else { "notifications.showList" };
                drop(workbench.execute_command(command, Vec::new(), window, cx));
            }))
            .child(self.hover_target("status.notifications", HoverContent::Text(tooltip), HoverOptions::status_bar()));
        Some(item)
    }

    /// notificationsCommands.ts's `MenuId.TitleBar` bell, shown for the top right position.
    pub(super) fn notifications_title_bar_item(&self, foreground: Rgba, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.notifications_position() != Position::TopRight || self.setting("workbench.notifications.showInTitleBar") == Value::Bool(false) {
            return None;
        }
        let item = self
            .icon("bell", 22., 16., foreground)
            .id("titlebar.notifications")
            .cursor_pointer()
            .on_click(cx.listener(|workbench, _: &ClickEvent, window, cx| drop(workbench.execute_command("notifications.toggleList", Vec::new(), window, cx))))
            .child(self.action_hover("Toggle Notifications", "notifications.toggleList", HoverOptions::instant()));
        Some(item)
    }

    fn notifications_tooltip(&self, in_progress: usize) -> String {
        let ui = &self.notifications;
        if ui.center_visible {
            return "Hide Notifications".into();
        }
        if ui.model.items.is_empty() {
            return "No Notifications".into();
        }
        let new = match ui.unread {
            0 => "No New Notifications".to_owned(),
            1 => "1 New Notification".to_owned(),
            count => format!("{count} New Notifications"),
        };
        match in_progress {
            0 => new,
            count => format!("{new} ({count} in progress)"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    BottomRight,
    BottomLeft,
    TopRight,
}

/// The toasts and the center sit `offset` from the window corner of `position`, clear of the status or title bar.
fn place<E: Styled>(element: E, position: Position, offset: f32) -> E {
    let element = element.absolute();
    match position {
        Position::BottomRight => element.right(px(offset)).bottom(px(STATUS_BAR_HEIGHT + offset)),
        Position::BottomLeft => element.left(px(offset)).bottom(px(STATUS_BAR_HEIGHT + offset)),
        Position::TopRight => element.right(px(offset)).top(px(TITLE_BAR_HEIGHT + offset)),
    }
}

/// `NotificationsToasts.PURGE_TIMEOUT`.
fn purge_timeout(severity: Severity) -> Duration {
    Duration::from_millis(match severity {
        Severity::Info => 10_000,
        Severity::Warning => 12_000,
        Severity::Error => 15_000,
    })
}

/// `--vscode-shadow-lg`.
fn shadow() -> BoxShadow {
    BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.), inset: false }
}

/// The toolbar's width: 22px actions, each with a 4px right margin.
fn toolbar_width(item: &Item, collapsed: bool) -> f32 {
    let actions = usize::from(!item.secondary.is_empty() || item.source_id.is_some()) + usize::from(item.can_collapse() && collapsed) + usize::from(!item.has_active_progress());
    actions as f32 * 26.
}

fn text_run(text: &str, window: &Window) -> TextRun {
    TextRun { len: text.len(), font: window.text_style().font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None }
}

fn text_width(text: &str, window: &Window) -> f32 {
    f32::from(window.text_system().shape_line(SharedString::from(text.to_owned()), px(13.), &[text_run(text, window)], None).width)
}

/// The offset helper's height in lines: the message wrapped at `width`.
fn message_lines(text: &str, width: f32, window: &Window) -> usize {
    let lines = window.text_system().shape_text(SharedString::from(text.to_owned()), px(13.), &[text_run(text, window)], Some(px(width)), None);
    lines.map(|lines| lines.iter().map(|line| line.wrap_boundaries().len() + 1).sum()).unwrap_or(1).max(1)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes.get(index + 1..index + 3).and_then(|hex| std::str::from_utf8(hex).ok()).and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
