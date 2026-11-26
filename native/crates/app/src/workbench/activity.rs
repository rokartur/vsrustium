//! workbench/services/activity (activityService.ts) and the badge of compositeBarActions.ts: each view container
//! collects activities (a view's `$setBadge`, progress shown on the container, the SCM and Extensions counts) and its
//! activity bar item shows one badge, progress first, then the sum of the number badges.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{Context, Div, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder, px, svg};
use serde_json::Value;

use super::Workbench;

/// progressService.ts `showOnActivityBar`: the badge waits for `options.delay` (300 by default) and stays at least
/// 300 ms once shown.
const PROGRESS_DELAY: Duration = Duration::from_millis(300);
const PROGRESS_MIN_TIME: Duration = Duration::from_millis(300);
/// progressService.css `.progress-badge > .badge-content::before`: the 13px sync glyph of its data URL mask.
const PROGRESS_ICON: &str = include_str!("activity/progress-badge.svg");

/// gpui draws svgs from paths: the glyph is written once to the temporary folder.
fn progress_icon_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("vsrustium-progress-badge-{}.svg", std::process::id()));
        std::fs::write(&path, PROGRESS_ICON).expect("the temporary folder is writable");
        path.to_string_lossy().into_owned()
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Badge {
    Number(u64),
    Progress,
}

/// A container's `ProgressBadge` from a running `withProgress`: shown after its delay, kept its minimum time.
struct ProgressActivity {
    container: String,
    shown: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Activities {
    progress: HashMap<u64, ProgressActivity>,
}

/// compositeBarActions.ts `getVisibleBadges` and `updateActivity`: progress wins over numbers; numbers add up and
/// show as `1K`, `1K+` past 999. `None` shows no badge.
pub(super) fn visible_badge(badges: &[Badge]) -> Option<Option<String>> {
    if badges.contains(&Badge::Progress) {
        return Some(None);
    }
    let total: u64 = badges.iter().map(|badge| if let Badge::Number(number) = badge { *number } else { 0 }).sum();
    if total == 0 {
        return None;
    }
    if total <= 999 {
        return Some(Some(total.to_string()));
    }
    let thousands = total as f64 / 1000.;
    let floor = thousands.floor();
    Some(Some(if thousands > floor { format!("{floor}K+") } else { format!("{floor}K") }))
}

impl Workbench {
    /// The view container of a view id, as `getViewContainerByViewId` for the views the port has.
    pub(super) fn view_container_of_view(&self, view: &str) -> Option<String> {
        match view {
            "workbench.explorer.fileView" | "outline" | "timeline" => return Some("workbench.view.explorer".into()),
            "workbench.scm" | "workbench.scm.repositories" | "workbench.scm.history" => return Some("workbench.view.scm".into()),
            _ => {}
        }
        let key = &self.tree_views.views.iter().find(|contributed| contributed.id == view)?.container;
        Some(match key.as_str() {
            "explorer" => "workbench.view.explorer".into(),
            "scm" => "workbench.view.scm".into(),
            "debug" => "workbench.view.debug".into(),
            "test" => "workbench.view.testing".into(),
            key => self.tree_views.containers.iter().find(|container| container.key == key)?.id.to_owned(),
        })
    }

    /// `getViewContainerActivities`: the container's badges this frame.
    pub(super) fn container_badges(&self, container: &str) -> Vec<Badge> {
        let mut badges = Vec::new();
        if self.activities.progress.values().any(|progress| progress.container == container && progress.shown.is_some()) {
            badges.push(Badge::Progress);
        }
        // viewPane.ts `updateBadge`: a view's badge goes to its container (`showViewActivity`).
        for view in &self.tree_views.views {
            if self.view_container_of_view(view.id).as_deref() != Some(container) {
                continue;
            }
            if let Some(value) = self.tree_view_badge(view.id).and_then(|badge| badge["value"].as_u64()) {
                badges.push(Badge::Number(value));
            }
        }
        match container {
            "workbench.view.explorer" => badges.push(Badge::Number(self.dirty_working_copy_count())),
            "workbench.view.scm" => badges.push(Badge::Number(self.scm_badge())),
            "workbench.view.extensions" => badges.push(Badge::Number(self.extensions_badge().map_or(0, |(count, _)| count as u64))),
            _ => {}
        }
        badges
    }

    /// activityaction.css `.badge .badge-content` (16px high, 9px semibold) at the Modern UI place (activityBar.css:
    /// 18px from the top, 3px from the right); the progress badge is a 16px circle with the sync glyph.
    pub(super) fn container_activity_badge(&self, container: &str) -> Option<Div> {
        let content = visible_badge(&self.container_badges(container))?;
        let foreground = self.color("activityBarBadge.foreground");
        let badge = div()
            .absolute()
            .top(px(18.))
            .right(px(3.))
            .h(px(16.))
            .rounded(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .bg(self.color("activityBarBadge.background"))
            .text_color(foreground)
            .when_some(self.theme.color("contrastBorder"), |badge, border| badge.border_1().border_color(border));
        Some(match content {
            Some(text) => badge.min_w(px(16.)).px(px(4.)).text_size(px(9.)).font_weight(gpui::FontWeight::SEMIBOLD).line_height(px(16.)).child(text),
            None => badge.w(px(16.)).child(svg().external_path(progress_icon_path()).size(px(13.)).text_color(foreground).into_any_element()),
        })
    }

    /// `showOnActivityBar` for `$startProgress` at a view container or view location (Explorer, Source Control,
    /// Extensions, or an id).
    pub(crate) fn start_activity_progress(&mut self, handle: u64, location: &Value, delay: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let container = match location {
            Value::Number(number) => match number.as_u64() {
                Some(1) => "workbench.view.explorer".to_owned(),
                Some(3) => "workbench.view.scm".to_owned(),
                Some(5) => "workbench.view.extensions".to_owned(),
                _ => return,
            },
            Value::String(id) if id.starts_with("workbench.view.") => id.clone(),
            Value::String(view) => match self.view_container_of_view(view) {
                Some(container) => container,
                None => return,
            },
            _ => return,
        };
        self.activities.progress.insert(handle, ProgressActivity { container, shown: None });
        let delay = delay.map_or(PROGRESS_DELAY, Duration::from_millis);
        cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(delay).await;
            let _ = workbench.update(cx, |workbench, cx| {
                let now = cx.background_executor().now();
                if let Some(progress) = workbench.activities.progress.get_mut(&handle) {
                    progress.shown = Some(now);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The progress ended: an unshown badge goes, a shown one after its minimum time.
    pub(crate) fn end_activity_progress(&mut self, handle: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(progress) = self.activities.progress.get(&handle) else { return };
        let Some(shown) = progress.shown else {
            self.activities.progress.remove(&handle);
            return;
        };
        let remaining = (shown + PROGRESS_MIN_TIME).saturating_duration_since(cx.background_executor().now());
        cx.spawn_in(window, async move |workbench, cx| {
            cx.background_executor().timer(remaining).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.activities.progress.remove(&handle);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{Badge, visible_badge};

    #[test]
    fn progress_wins_and_numbers_add_up() {
        assert_eq!(visible_badge(&[Badge::Number(2), Badge::Progress]), Some(None));
        assert_eq!(visible_badge(&[Badge::Number(2), Badge::Number(3)]), Some(Some("5".into())));
        assert_eq!(visible_badge(&[Badge::Number(0)]), None);
        assert_eq!(visible_badge(&[]), None);
    }

    #[test]
    fn numbers_past_999_show_thousands() {
        assert_eq!(visible_badge(&[Badge::Number(999)]), Some(Some("999".into())));
        assert_eq!(visible_badge(&[Badge::Number(1000)]), Some(Some("1K".into())));
        assert_eq!(visible_badge(&[Badge::Number(1500)]), Some(Some("1K+".into())));
    }
}
