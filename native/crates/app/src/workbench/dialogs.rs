//! workbench/services/dialogs and the dialog handler contribution (dialog.contribution.ts): `window.dialogStyle`
//! picks the native message box or the custom dialog, and `workbench.action.showAboutDialog` with
//! `createNativeAboutDialogDetails`.

use futures::channel::oneshot;
use gpui::{Context, PromptLevel, Window};

use super::Workbench;
use super::dialog_view::{CustomDialog, DialogButton};
use crate::commands::Outcome;
use crate::dialog;

impl Workbench {
    pub(super) fn register_dialog_commands(&mut self) {
        self.commands.register("workbench.action.showAboutDialog", |workbench, _, window, cx| {
            workbench.show_about_dialog(window, cx);
            Outcome::done()
        });
    }

    /// `useCustomDialog`: `window.dialogStyle` is `custom`.
    pub(crate) fn uses_custom_dialogs(&self) -> bool {
        self.setting("window.dialogStyle") == "custom"
    }

    /// The browser dialog handler's `prompt` for a message box: the severity's codicon, the message, the detail, the
    /// buttons with the last as cancel when there is more than one.
    pub(crate) fn show_custom_message_box(&mut self, level: PromptLevel, message: String, detail: Option<String>, buttons: Vec<String>, checkbox: Option<String>, cx: &mut Context<Self>) -> oneshot::Receiver<(usize, bool)> {
        let icon = match level {
            PromptLevel::Info => "info",
            PromptLevel::Warning => "warning",
            PromptLevel::Critical => "error",
        };
        let cancel = buttons.len().checked_sub(1);
        let buttons = buttons.into_iter().map(|label| DialogButton { label, detail: None }).collect();
        let answer = self.show_custom_dialog(CustomDialog { icon, message, detail, details: Vec::new(), checkbox, buttons, cancel }, cx);
        let (sender, receiver) = oneshot::channel();
        cx.background_executor()
            .spawn(async move {
                if let Ok(result) = answer.await {
                    let _ = sender.send((result.button, result.checkbox_checked));
                }
            })
            .detach();
        receiver
    }

    /// `dialogService.about`: the product name and its details with Copy and OK; Copy puts the details on the clipboard.
    fn show_about_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // DialogService.about refuses while extension tests run.
        if self.skip_dialogs {
            return;
        }
        let details = about_details(&os_properties());
        let answer = dialog::show_message_box(window, PromptLevel::Info, crate::product::name_long(), Some(&format!("\n{details}")), &["Copy", "OK"], None, cx);
        cx.spawn(async move |_, cx| {
            if let Ok((0, _)) = answer.await {
                let _ = cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(details)));
            }
        })
        .detach();
    }
}

/// `IOSProperties` as Node's `os` gives them: type, arch and release.
fn os_properties() -> (String, String, String) {
    // SAFETY: uname fills the zeroed struct with NUL-terminated strings.
    let release = unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        libc::uname(&mut name);
        std::ffi::CStr::from_ptr(name.release.as_ptr()).to_string_lossy().into_owned()
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    ("Darwin".into(), arch.into(), release)
}

/// `createNativeAboutDialogDetails`: the version, commit, date and OS.
// Product decision: no Electron, ElectronBuildId, Chromium, Node.js or V8 lines (the port has no Electron) and no
// @github/copilot lines (AI removed).
fn about_details((os_type, arch, release): &(String, String, String)) -> String {
    let product = |key: &str| crate::product::get(key).as_str().filter(|value| !value.is_empty()).map(str::to_owned);
    let version = product("version").unwrap_or_default();
    let version = match product("darwinUniversalAssetId") {
        Some(_) => format!("{version} (Universal)"),
        None => version,
    };
    let commit = product("commit").unwrap_or_else(|| "Unknown".into());
    let date = product("date").unwrap_or_else(|| "Unknown".into());
    format!("Version: {version}\nCommit: {commit}\nDate: {date}\nOS: {os_type} {arch} {release}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_details_name_version_commit_date_and_os() {
        crate::product::load(std::path::Path::new(env!("VSCODE_SRC")));
        let details = about_details(&("Test OS".into(), "test-arch".into(), "1.0".into()));
        let lines: Vec<&str> = details.lines().collect();
        assert!(lines[0].starts_with("Version: 1.147"));
        assert_eq!(&lines[1..], ["Commit: Unknown", "Date: Unknown", "OS: Test OS test-arch 1.0"]);
    }
}
