//! Zen mode and the status bar's visibility of browser/layout.ts (`toggleZenMode`, `setStatusBarHidden`,
//! `ZEN_MODE_ACTIVE`, `ZEN_MODE_EXIT_INFO`) and the zen mode actions of workbench.zenMode.contribution.ts.

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::layout::Part;
use crate::commands::Outcome;

/// `LayoutStateKeys.ZEN_MODE_ACTIVE` and `ZEN_MODE_EXIT_INFO` in workspace storage.
const ACTIVE_KEY: &str = "workbench.zenMode.active";
const EXIT_INFO_KEY: &str = "workbench.zenMode.exitInfo";

#[derive(Default)]
pub(super) struct ZenMode {
    pub(super) active: bool,
    /// Read once, when the grid is first laid out (`restoreParts`).
    restored: bool,
}

impl Workbench {
    pub(super) fn register_zen_mode_actions(&mut self) {
        self.commands.register("workbench.action.toggleZenMode", |workbench, _, window, cx| {
            workbench.toggle_zen_mode(false, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.exitZenMode", |workbench, _, window, cx| {
            if workbench.zen_mode.active {
                workbench.toggle_zen_mode(false, window, cx);
            }
            Outcome::done()
        });
        // layoutActions.ts `ToggleStatusbarVisibilityAction` writes `workbench.statusBar.visible`.
        self.commands.register("workbench.action.toggleStatusbarVisibility", |workbench, _, _, cx| {
            let visible = workbench.setting("workbench.statusBar.visible") != json!(false);
            workbench.update_setting("workbench.statusBar.visible", json!(!visible));
            cx.notify();
            Outcome::done()
        });
    }

    /// `isVisible(Parts.STATUSBAR_PART)`: `workbench.statusBar.visible`, hidden in zen mode with `zenMode.hideStatusBar`.
    pub(super) fn is_status_bar_visible(&self) -> bool {
        if self.zen_mode.active && self.setting("zenMode.hideStatusBar") != json!(false) {
            return false;
        }
        self.setting("workbench.statusBar.visible") != json!(false)
    }

    /// Zen mode with `zenMode.hideActivityBar` hides the activity bar without changing its setting.
    pub(super) fn zen_mode_hides_activity_bar(&self) -> bool {
        self.zen_mode.active && self.setting("zenMode.hideActivityBar") != json!(false)
    }

    /// `zenMode.hideLineNumbers` turns the editors' line numbers off while zen mode is on.
    pub(super) fn zen_mode_hides_line_numbers(&self) -> bool {
        self.zen_mode.active && self.setting("zenMode.hideLineNumbers") != json!(false)
    }

    /// The `restoreParts` step: a window closed in zen mode opens in it again with `zenMode.restore`, else leaves it
    /// and shows the parts it hid.
    pub(super) fn restore_zen_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.zen_mode.restored, true) {
            return;
        }
        let was_active = self.workspace_storage.lock().unwrap().get(ACTIVE_KEY).is_some_and(|value| value == "true");
        if !was_active {
            return;
        }
        let restore = self.setting("zenMode.restore") != json!(false);
        self.zen_mode.active = !restore;
        self.toggle_zen_mode(true, window, cx);
    }

    /// `toggleZenMode`: hides the side bars and the panel, and per `zenMode.*` the activity bar, the status bar and the
    /// line numbers, silences notifications, centers the editor and enters full screen; leaving puts back what it changed.
    pub(super) fn toggle_zen_mode(&mut self, restoring: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.zen_mode.active = !self.zen_mode.active;
        self.workspace_storage.lock().unwrap().set(ACTIVE_KEY, self.zen_mode.active.to_string());
        let stored: Option<Value> = self.workspace_storage.lock().unwrap().get(EXIT_INFO_KEY).and_then(|value| serde_json::from_str(value).ok());
        let mut exit_info = stored.unwrap_or_else(|| json!({ "transitionedToCenteredEditorLayout": false, "transitionedToFullScreen": false, "handleNotificationsDoNotDisturbMode": false, "wasVisible": { "auxiliaryBar": false, "panel": false, "sideBar": false } }));
        let enabled = |workbench: &Self, key: &str| workbench.setting(key) != json!(false);
        let toggle_full_screen;
        if self.zen_mode.active {
            toggle_full_screen = !window.is_fullscreen() && enabled(self, "zenMode.fullScreen");
            if !restoring {
                exit_info["transitionedToFullScreen"] = json!(toggle_full_screen);
                exit_info["transitionedToCenteredEditorLayout"] = json!(!self.is_main_editor_centered() && enabled(self, "zenMode.centerLayout"));
                exit_info["handleNotificationsDoNotDisturbMode"] = json!(!self.notifications.model.do_not_disturb);
                exit_info["wasVisible"] = json!({
                    "sideBar": self.is_part_visible(Part::SideBar),
                    "panel": self.is_part_visible(Part::Panel),
                    "auxiliaryBar": self.is_part_visible(Part::AuxiliaryBar),
                });
                self.workspace_storage.lock().unwrap().set(EXIT_INFO_KEY, exit_info.to_string());
            }
            self.set_part_hidden(Part::Panel, true, cx);
            self.set_part_hidden(Part::AuxiliaryBar, true, cx);
            self.set_part_hidden(Part::SideBar, true, cx);
            if enabled(self, "zenMode.silentNotifications") && exit_info["handleNotificationsDoNotDisturbMode"] == json!(true) {
                self.set_do_not_disturb(true, window, cx);
            }
            if enabled(self, "zenMode.centerLayout") {
                self.center_main_editor_layout(true, cx);
            }
        } else {
            for (key, part) in [("panel", Part::Panel), ("auxiliaryBar", Part::AuxiliaryBar), ("sideBar", Part::SideBar)] {
                if exit_info["wasVisible"][key] == json!(true) {
                    self.set_part_hidden(part, false, cx);
                }
            }
            if exit_info["transitionedToCenteredEditorLayout"] == json!(true) {
                self.center_main_editor_layout(false, cx);
            }
            if exit_info["handleNotificationsDoNotDisturbMode"] == json!(true) {
                self.set_do_not_disturb(false, window, cx);
            }
            toggle_full_screen = exit_info["transitionedToFullScreen"] == json!(true) && window.is_fullscreen();
        }
        if toggle_full_screen {
            window.toggle_fullscreen();
        }
        window.focus(&self.editor_focus.clone(), cx);
        cx.notify();
    }
}
