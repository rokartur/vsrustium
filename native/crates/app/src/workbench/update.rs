//! The update channel of the port's own builds (platform/update, workbench/services/update, workbench/contrib/update):
//! product.json `updateUrl` names a GitHub "latest release" API URL (`https://api.github.com/repos/<owner>/<repo>/
//! releases/latest`). Without it the service is disabled, as AbstractUpdateService is without `updateUrl`; the
//! port's product.json has none until a release is published. `update.mode` picks the checks: `none` never,
//! `manual` from Check for Updates..., `start` 30 s after startup too, `default` also every hour. Nothing is
//! downloaded: Download Update opens the release page, where the user downloads the app.

use std::time::Duration;

use gpui::{Context, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::notifications::Severity;

/// abstractUpdateService.ts: the first check 30 s after startup, then hourly with `update.mode: default`.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// A release newer than the running version.
#[derive(Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
}

/// The version of a GitHub release from its tag (`v1.2.3` or `1.2.3`) and its page, when newer than `current`.
pub fn newer_release(latest: &Value, current: &str) -> Option<Release> {
    let tag = latest["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v');
    if latest["draft"] == true || latest["prerelease"] == true || !is_newer(version, current) {
        return None;
    }
    // The page is opened with the system's handler: only a web page qualifies, never a file or app URL.
    let page = latest["html_url"].as_str().filter(|page| page.starts_with("https://"))?;
    Some(Release { version: version.to_owned(), page: page.to_owned() })
}

/// Compares the dotted numbers of two versions; a pre-release suffix (`-beta`) makes nothing newer by itself.
fn is_newer(candidate: &str, current: &str) -> bool {
    let numbers = |version: &str| -> Vec<u64> { version.split(['-', '+']).next().unwrap_or_default().split('.').map(|part| part.parse().unwrap_or(0)).collect() };
    numbers(candidate) > numbers(current)
}

fn update_url() -> Option<&'static str> {
    crate::product::get("updateUrl").as_str()
}

/// The latest release through curl (the port has no HTTP client of its own); `None` on any failure, logged.
fn fetch_latest(url: &str) -> Option<Value> {
    let output = std::process::Command::new("/usr/bin/curl").args(["-fsSL", "--max-time", "30", "-H", "Accept: application/vnd.github+json", url]).output();
    match output {
        Ok(output) if output.status.success() => serde_json::from_slice(&output.stdout).ok(),
        Ok(output) => {
            eprintln!("[update] checking {url}: {}", String::from_utf8_lossy(&output.stderr).trim());
            None
        }
        Err(error) => {
            eprintln!("[update] checking {url}: {error}");
            None
        }
    }
}

impl Workbench {
    pub fn register_update_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commands.register("update.checkForUpdate", |workbench, _, window, cx| {
            if update_url().is_none() || workbench.setting("update.mode") == "none" {
                // CONTEXT_UPDATE_STATE is `disabled`: the precondition fails and the command does nothing.
                return Outcome::done();
            }
            workbench.check_for_updates(true, window, cx);
            Outcome::done()
        });
        if update_url().is_none() {
            return;
        }
        cx.spawn_in(window, async move |workbench, cx| {
            let mut wait = FIRST_CHECK;
            loop {
                cx.background_executor().timer(wait).await;
                let Ok(mode) = workbench.update(cx, |workbench, _| workbench.setting("update.mode")) else { return };
                if mode == "start" || mode == "default" {
                    let _ = workbench.update_in(cx, |workbench, window, cx| workbench.check_for_updates(false, window, cx));
                }
                if mode != "default" {
                    return;
                }
                wait = CHECK_INTERVAL;
            }
        })
        .detach();
    }

    /// `checkForUpdates(explicit)`: an explicit check says when there is nothing new; a found release asks to open
    /// its page (`AvailableForDownload` on a platform that cannot update itself).
    fn check_for_updates(&mut self, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = update_url() else { return };
        let current = crate::product::get("version").as_str().unwrap_or_default().to_owned();
        let latest = cx.background_executor().spawn(async move { fetch_latest(url) });
        cx.spawn_in(window, async move |workbench, cx| {
            let release = latest.await.and_then(|latest| newer_release(&latest, &current));
            let _ = workbench.update_in(cx, |workbench, window, cx| match release {
                None if explicit => {
                    let _ = window.prompt(PromptLevel::Info, "There are currently no updates available.", None, &["OK"], cx);
                }
                None => {}
                Some(release) => {
                    let name = crate::product::name_long();
                    let reply = workbench.prompt_notification(Severity::Info, format!("There is an available update ({}) for {name}.", release.version), &["Download Update", "Later"]);
                    cx.spawn(async move |_, cx| {
                        if let Ok(Ok(choice)) = reply.await {
                            if choice == 0 {
                                let _ = cx.update(|cx| cx.open_url(&release.page));
                            }
                        }
                    })
                    .detach();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_newer_published_release_counts() {
        let release = |tag: &str| json!({ "tag_name": tag, "html_url": "https://github.com/rokartur/vsrustium/releases/tag/x", "draft": false, "prerelease": false });
        assert_eq!(newer_release(&release("v1.148.0"), "1.147.0").map(|release| release.version), Some("1.148.0".into()));
        assert_eq!(newer_release(&release("1.147.1"), "1.147.0").map(|release| release.version), Some("1.147.1".into()));
        assert_eq!(newer_release(&release("v1.147.0"), "1.147.0"), None);
        assert_eq!(newer_release(&release("v1.99.9"), "1.147.0"), None);
        let mut draft = release("v2.0.0");
        draft["draft"] = json!(true);
        assert_eq!(newer_release(&draft, "1.147.0"), None);
        assert_eq!(newer_release(&json!({ "message": "Not Found" }), "1.147.0"), None);
        let mut local = release("v2.0.0");
        local["html_url"] = json!("file:///Applications/Calculator.app");
        assert_eq!(newer_release(&local, "1.147.0"), None);
    }
}
