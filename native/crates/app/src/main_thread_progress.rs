//! The main-thread side of `window.withProgress`: workbench/api/browser/mainThreadProgress.ts with the notification
//! and window locations of workbench/services/progress/browser/progressService.ts.

use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::command_palette::strip_icons;
use crate::commands::Outcome;
use crate::main_thread_status_bar::StatusEntry;
use crate::notifications::{Action, Notification, Priority, Progress, Run, Severity};
use crate::workbench::Workbench;

/// platform/progress/common/progress.ts `ProgressLocation`.
const LOCATION_WINDOW: u64 = 10;
const LOCATION_NOTIFICATION: u64 = 15;
/// `withProgress`: window progress waits this long before its silent notification shows; `withWindowProgress`
/// waits as long before the status entry shows and keeps it at least as long.
const WINDOW_DELAY: Duration = Duration::from_millis(150);
/// `withNotificationProgress`: a progress notification without a delay stays at least this long.
const MIN_NOTIFICATION_TIME: Duration = Duration::from_millis(800);

/// A running `$startProgress` task: the `progressStateModel` of `withNotificationProgress`.
pub struct ProgressTask {
    title: Option<String>,
    /// The latest step's message, which `updateNotification` joins to the title.
    step_message: Option<String>,
    /// The latest step message that had one, which the window progress keeps showing.
    window_message: Option<String>,
    source: Option<String>,
    source_id: Option<String>,
    cancellable: Option<String>,
    extension_id: Option<String>,
    priority: Priority,
    delayed: bool,
    started: Instant,
    /// Set once the delayed notification is scheduled; its increment is the step that scheduled it.
    scheduled: bool,
    /// The notification's key once it shows, and when.
    notification: Option<(u64, Instant)>,
    /// Ended, while the window progress status entry stays its minimum time.
    ended: bool,
}

impl ProgressTask {
    /// `updateNotification`'s `titleAndMessage`.
    fn title_and_message(&self) -> Option<String> {
        match (&self.title, &self.step_message) {
            (Some(title), Some(message)) => Some(format!("{title}: {message}")),
            (title, message) => title.clone().or_else(|| message.clone()),
        }
    }
}

impl Workbench {
    /// `$startProgress(handle, options, extensionId)`.
    pub(crate) fn start_progress(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let handle = args[0].as_u64().unwrap_or_default();
        let options = &args[1];
        let (priority, delayed) = match options["location"].as_u64() {
            Some(LOCATION_NOTIFICATION) => (Priority::Default, false),
            // Window progress from the API has no command, so it is a silent notification first shown in the status bar.
            Some(LOCATION_WINDOW) => (Priority::Silent, true),
            // progressService.ts `withPaneCompositeProgress` and `withViewProgress`: the activity bar badge.
            _ => {
                self.start_activity_progress(handle, &options["location"], options["delay"].as_u64(), window, cx);
                return Outcome::done();
            }
        };
        let cancellable = match &options["cancellable"] {
            Value::String(label) => Some(label.clone()),
            Value::Bool(true) => Some("Cancel".to_owned()),
            _ => None,
        };
        let task = ProgressTask {
            title: options["title"].as_str().map(str::to_owned),
            step_message: None,
            window_message: None,
            source: options["source"]["label"].as_str().or(options["source"].as_str()).map(str::to_owned),
            source_id: options["source"]["id"].as_str().map(str::to_owned),
            cancellable,
            extension_id: args.get(2).and_then(Value::as_str).filter(|_| !delayed).map(str::to_owned),
            priority,
            delayed,
            started: cx.background_executor().now(),
            scheduled: false,
            notification: None,
            ended: false,
        };
        self.notifications.progress.insert(handle, task);
        self.update_progress_notification(handle, None, window, cx);
        Outcome::done()
    }

    /// `$progressReport(handle, { message, increment })`.
    pub(crate) fn report_progress(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let handle = args[0].as_u64().unwrap_or_default();
        let Some(task) = self.notifications.progress.get_mut(&handle) else { return Outcome::done() };
        let message = args[1]["message"].as_str().filter(|message| !message.is_empty()).map(str::to_owned);
        if message.is_some() {
            task.window_message = message.clone();
        }
        task.step_message = message;
        self.update_progress_notification(handle, args[1]["increment"].as_f64(), window, cx);
        Outcome::done()
    }

    /// `$progressEnd(handle)`: the notification closes, after `MIN_NOTIFICATION_TIME` from the start when it showed
    /// without a delay; a shown window progress entry stays `WINDOW_DELAY` at least.
    pub(crate) fn end_progress(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let handle = args[0].as_u64().unwrap_or_default();
        self.end_activity_progress(handle, window, cx);
        let Some(task) = self.notifications.progress.get_mut(&handle) else { return Outcome::done() };
        task.ended = true;
        let now = cx.background_executor().now();
        let close_at = match task.delayed {
            true => now,
            false => task.started + MIN_NOTIFICATION_TIME,
        };
        let notification = task.notification.map(|(key, _)| key);
        let entry_until = task.notification.map_or(now, |(_, shown)| shown + WINDOW_DELAY * 2);
        cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(entry_until.saturating_duration_since(cx.background_executor().now())).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.notifications.progress.remove(&handle);
                cx.notify();
            });
        })
        .detach();
        if let Some(key) = notification {
            cx.spawn_in(window, async move |workbench, cx| {
                cx.background_executor().timer(close_at.saturating_duration_since(cx.background_executor().now())).await;
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.close_notification(key, window, cx));
            })
            .detach();
        }
        cx.notify();
        Outcome::done()
    }

    /// `updateNotification`: creates the notification once there is text, after the delay for window progress, else
    /// updates its message and progress.
    fn update_progress_notification(&mut self, handle: u64, increment: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.notifications.progress.get_mut(&handle) else { return };
        let Some(text) = task.title_and_message() else { return };
        let Some((key, _)) = task.notification else {
            if !task.delayed {
                self.show_progress_notification(handle, increment, cx);
            } else if !task.scheduled {
                task.scheduled = true;
                cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(WINDOW_DELAY).await;
                    let _ = workbench.update(cx, |workbench, cx| workbench.show_progress_notification(handle, increment, cx));
                })
                .detach();
            }
            return;
        };
        let Some(item) = self.notifications.model.item_mut(key) else { return };
        item.update_message(&strip_icons(&text));
        if let Some(increment) = increment {
            let progress = item.progress.get_or_insert_with(Progress::default);
            if increment >= 0. {
                progress.set_total(100.);
                progress.add_worked(increment);
            } else {
                progress.set_infinite();
            }
        }
        cx.notify();
    }

    /// `createNotification`: an info notification with the progress, Cancel when cancellable, and Manage Extension.
    fn show_progress_notification(&mut self, handle: u64, increment: Option<f64>, cx: &mut Context<Self>) {
        let Some(task) = self.notifications.progress.get(&handle).filter(|task| !task.ended && task.notification.is_none()) else { return };
        let Some(text) = task.title_and_message() else { return };
        let mut notification = Notification::new(Severity::Info, strip_icons(&text));
        notification.legacy_links = true;
        notification.priority = task.priority;
        notification.source = task.source.clone();
        notification.source_id = task.source_id.clone();
        notification.progress = Some(match increment.filter(|increment| *increment >= 0.) {
            Some(worked) => Progress { total: Some(100.), worked: Some(worked), ..Progress::default() },
            None => Progress { infinite: true, ..Progress::default() },
        });
        if let Some(label) = &task.cancellable {
            notification.primary.push(Action { id: "progress.cancel".into(), label: label.clone(), run: Run::Reply(0) });
        }
        if let Some(id) = &task.extension_id {
            notification.secondary.push(Action { id: id.clone(), label: "Manage Extension".into(), run: Run::Command("_extensions.manage".into(), vec![json!(id)]) });
        }
        let Some(key) = self.notify(notification) else { return };
        let (sender, receiver) = oneshot::channel();
        self.notifications.replies.insert(key, sender);
        if let Some(task) = self.notifications.progress.get_mut(&handle) {
            task.notification = Some((key, cx.background_executor().now()));
        }
        cx.spawn(async move |workbench, cx| {
            let picked = receiver.await;
            let _ = workbench.update(cx, |workbench, cx| {
                // `progressStateModel.cancel`: the extension's token cancels and the task is done.
                if picked == Ok(Ok(json!(0))) {
                    workbench.notifications.progress.remove(&handle);
                    drop(workbench.host.call("ExtHostProgress", "$acceptProgressCanceled", vec![json!(handle)]));
                }
                cx.notify();
            });
        })
        .detach();
        // `withWindowProgress` shows the status entry of a hidden notification after its delay.
        cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(WINDOW_DELAY).await;
            let _ = workbench.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    /// `updateWindowProgress`: the newest progress whose notification is neither a toast nor in the open center, as
    /// the `status.progress` entry that shows the notifications on click.
    pub(crate) fn window_progress_entry(&self, now: Instant) -> Option<StatusEntry> {
        let ui = &self.notifications;
        let mut tasks: Vec<_> = ui.progress.values().filter_map(|task| Some((task, task.notification?))).collect();
        tasks.sort_by_key(|(_, (_, shown))| std::cmp::Reverse(*shown));
        let (task, _) = tasks.into_iter().find(|(task, (key, shown))| {
            let open = ui.model.item(*key).is_some();
            let hidden = !open || !(ui.center_visible || ui.toast_shown(*key));
            hidden && now >= *shown + WINDOW_DELAY && (open || task.ended)
        })?;
        let title = task.title.as_deref().map(strip_icons);
        let message = task.window_message.as_deref().map(strip_icons);
        let (text, tooltip) = match (title, message, &task.source) {
            (Some(title), Some(message), source) => {
                let text = format!("{title}: {message}");
                (text.clone(), source.as_ref().map_or(text, |source| format!("[{source}] {title}: {message}")))
            }
            (Some(text), None, source) | (None, Some(text), source) => (text.clone(), source.as_ref().map_or(text.clone(), |source| format!("[{source}]: {text}"))),
            (None, None, _) => return None,
        };
        Some(StatusEntry {
            entry_id: "status.progress".into(),
            id: "status.progress".into(),
            name: "Progress Message".into(),
            text,
            tooltip: Some(strip_icons(&tooltip).trim().to_owned()),
            command: Some(json!("notifications.showList")),
            color: None,
            background_color: None,
            kind: None,
            align_left: true,
            priority: f64::MIN,
            secondary: 0,
            show_progress: true,
        })
    }
}
