//! workbench/contrib/relauncher: `SettingsChangeRelauncher` asks to restart when a setting that applies only at
//! startup changes, and `hostService.restart` relaunches the app with its arguments.

use gpui::{Context, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use super::explorer_actions::confirm;

/// The settings `SettingsChangeRelauncher.update` observes on macOS; the chat and agent host ones are left out with
/// AI (port/notes.md). `window.titleBarStyle` counts only for `native` and `custom`.
const RELAUNCH_SETTINGS: [&str; 9] = [
    "window.titleBarStyle",
    "window.nativeTabs",
    "window.nativeFullScreen",
    "window.clickThroughInactive",
    "security.workspace.trust.enabled",
    "security.restrictUNCAccess",
    "accessibility.verbosity.debug",
    "workbench.enableExperiments",
    "telemetry.feedback.enabled",
];
/// Observed while the product's quality is not `stable` (the port's product.json has no quality).
const PPE_SETTING: &str = "_extensionsGallery.enablePPE";

/// `ChangeObserver` per setting: the last value seen, `None` before the first.
#[derive(Default)]
pub struct Relauncher {
    seen: Vec<Option<Value>>,
}

impl Relauncher {
    /// `ChangeObserver.handleChange` over every observed value: true when one changed after the first look.
    pub fn handle_change(&mut self, values: Vec<Value>) -> bool {
        if self.seen.is_empty() {
            self.seen = vec![None; values.len()];
        }
        let mut changed = false;
        for (seen, value) in self.seen.iter_mut().zip(values) {
            changed |= seen.as_ref().is_some_and(|seen| *seen != value);
            *seen = Some(value);
        }
        changed
    }
}

impl Workbench {
    /// Runs on every render, so every settings write path reaches it, as `onDidChangeConfiguration` does upstream.
    /// shortcut: a changed default (an extension contributing one) also asks, where upstream skips DEFAULT-sourced
    /// changes; none of the observed settings has an extension default in the built-ins.
    pub(super) fn check_relaunch_settings(&mut self, window: &Window, cx: &mut Context<Self>) {
        let mut values: Vec<Value> = RELAUNCH_SETTINGS
            .iter()
            .map(|key| match (*key, self.setting(key)) {
                ("window.titleBarStyle", style) if style != "native" && style != "custom" => Value::Null,
                (_, value) => value,
            })
            .collect();
        values.push(self.setting(PPE_SETTING));
        if !self.relauncher.handle_change(values) || !window.is_window_active() {
            return;
        }
        let name = crate::product::name_long();
        let message = "A setting has changed that requires a restart to take effect.".to_owned();
        let detail = format!("Press the restart button to restart {name} and enable the setting.");
        cx.spawn_in(window, async move |workbench, cx| {
            if confirm(&workbench, cx, PromptLevel::Info, message, Some(detail), "Restart").await {
                let _ = cx.update(|_, cx| relaunch(cx));
            }
        })
        .detach();
    }
}

/// `INativeHostService.relaunch`: the same executable and arguments start again, then this process quits.
pub fn relaunch(cx: &mut gpui::App) {
    let mut args = std::env::args_os();
    let executable = args.next().expect("the executable");
    match std::process::Command::new(executable).args(args).spawn() {
        Ok(_) => cx.quit(),
        Err(error) => eprintln!("relaunching: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::Relauncher;
    use serde_json::json;

    #[test]
    fn only_a_change_after_the_first_look_asks() {
        let mut relauncher = Relauncher::default();
        assert!(!relauncher.handle_change(vec![json!(false), json!("custom")]));
        assert!(!relauncher.handle_change(vec![json!(false), json!("custom")]));
        assert!(relauncher.handle_change(vec![json!(true), json!("custom")]));
        assert!(!relauncher.handle_change(vec![json!(true), json!("custom")]));
    }
}
