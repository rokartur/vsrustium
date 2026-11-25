//! userActivityService.ts with domActivityTracker.ts: the window counts as active while key or mouse presses keep
//! coming, checked every 30 s; two checks without one release it and it turns inactive 10 s later. The extension host
//! hears the change as `window.state.active` (mainThreadWindow.ts).
//! shortcut: progressService.ts's mark for a progress held over 15 s is missing; add it with the progress service.

use std::time::Duration;

use gpui::{Context, Task};
use serde_json::json;

use super::Workbench;

const CHECK_INTERVAL: Duration = Duration::from_secs(30);
const MIN_INTERVALS_WITHOUT_ACTIVITY: u32 = 2;
const MARK_INACTIVE_DEBOUNCE: Duration = Duration::from_secs(10);

pub struct UserActivity {
    is_active: bool,
    intervals_without_activity: u32,
    interval: Option<Task<()>>,
    mark_inactive: Option<Task<()>>,
}

impl Default for UserActivity {
    fn default() -> Self {
        UserActivity { is_active: true, intervals_without_activity: MIN_INTERVALS_WITHOUT_ACTIVITY, interval: None, mark_inactive: None }
    }
}

impl Workbench {
    /// `onActivity`: a key or mouse press anywhere in the window.
    pub(super) fn user_activity(&mut self, cx: &mut Context<Self>) {
        if self.user_activity.intervals_without_activity == MIN_INTERVALS_WITHOUT_ACTIVITY {
            self.user_activity.mark_inactive = None;
            self.set_user_active(true);
            self.user_activity.interval = Some(cx.spawn(async move |workbench, cx| {
                loop {
                    cx.background_executor().timer(CHECK_INTERVAL).await;
                    match workbench.update(cx, |workbench, cx| workbench.activity_interval(cx)) {
                        Ok(false) => {}
                        _ => return,
                    }
                }
            }));
        }
        self.user_activity.intervals_without_activity = 0;
    }

    /// One check without activity; the second releases the active mark. Returns whether the checks stop.
    fn activity_interval(&mut self, cx: &mut Context<Self>) -> bool {
        self.user_activity.intervals_without_activity += 1;
        if self.user_activity.intervals_without_activity != MIN_INTERVALS_WITHOUT_ACTIVITY {
            return false;
        }
        self.user_activity.mark_inactive = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(MARK_INACTIVE_DEBOUNCE).await;
            workbench.update(cx, |workbench, _| workbench.set_user_active(false)).ok();
        }));
        true
    }

    fn set_user_active(&mut self, active: bool) {
        if self.user_activity.is_active != active {
            self.user_activity.is_active = active;
            self.host.send("ExtHostWindow", "$onDidChangeWindowActive", vec![json!(active)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::TestAppContext;
    use serde_json::{Value, json};

    use super::*;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    #[gpui::test]
    fn the_window_turns_inactive_after_two_quiet_checks_and_the_debounce(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let (sender, messages) = std::sync::mpsc::channel();
        let (_, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx));
        let active_changes = || messages.try_iter().filter(|message| message.method == "$onDidChangeWindowActive").map(|message| message.args).collect::<Vec<Vec<Value>>>();
        cx.run_until_parked();

        cx.executor().advance_clock(CHECK_INTERVAL * 2 + MARK_INACTIVE_DEBOUNCE - Duration::from_secs(1));
        assert!(active_changes().is_empty());
        cx.executor().advance_clock(Duration::from_secs(1));
        assert_eq!(active_changes(), [vec![json!(false)]]);

        cx.simulate_keystrokes("x");
        assert_eq!(active_changes(), [vec![json!(true)]]);
        cx.executor().advance_clock(CHECK_INTERVAL + Duration::from_secs(20));
        cx.simulate_keystrokes("x");
        // Without that press the window would turn inactive at 70 s; with it, at 100 s.
        cx.executor().advance_clock(CHECK_INTERVAL + Duration::from_secs(19));
        assert!(active_changes().is_empty(), "a press between checks keeps the window active");
    }
}
