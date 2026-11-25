//! workspace.contribution.ts `WorkspaceTrustUXHandler`: the Restricted Mode status entry and banner of an untrusted
//! window, and the Workspaces trust commands.

use futures::channel::oneshot;
use gpui::{ClickEvent, Context, Div, InteractiveElement, MouseButton, ParentElement, Rgba, Stateful, StatefulInteractiveElement, Styled, Window, div, px};
use serde_json::{Value, json};

use super::Workbench;
use super::dialog_view::{CustomDialog, DialogButton, DialogResult};
use super::host_requests::uri_parts;
use super::hover::{HoverContent, HoverOptions};
use super::menubar::NAME_LONG;
use crate::commands::Outcome;
use crate::main_thread_status_bar::StatusEntry;
use crate::quick_input::path_label;
use crate::workspace_trust::{BANNER_DISMISSED_KEY, STARTUP_PROMPT_SHOWN_KEY};

pub(super) const MANAGE_TRUST_COMMAND_ID: &str = "workbench.trust.manage";
const CONFIGURE_TRUST_COMMAND_ID: &str = "workbench.trust.configure";
const LIST_UNSUPPORTED_EXTENSIONS_COMMAND_ID: &str = "workbench.extensions.action.listWorkspaceUnsupportedExtensions";
const LEARN_MORE_URL: &str = "https://aka.ms/vscode-workspace-trust";

impl Workbench {
    pub(super) fn register_workspace_trust_commands(&mut self) {
        self.commands.register(CONFIGURE_TRUST_COMMAND_ID, |workbench, _, window, cx| {
            workbench.execute_command("workbench.action.openSettings", vec![json!({ "query": "@tag:workspaceTrust" })], window, cx)
        });
    }

    /// `showModalOnStart`: the startup prompt of an untrusted folder or workspace, per
    /// `security.workspace.trust.startupPrompt`.
    pub(crate) fn show_trust_modal_on_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace() else { return };
        if !self.workspace_trust.enabled || self.workspace_trust.is_trusted() {
            return;
        }
        let prompt = self.setting("security.workspace.trust.startupPrompt");
        let shown = self.workspace_storage.lock().unwrap().get(STARTUP_PROMPT_SHOWN_KEY) == Some("true");
        if prompt == "never" || (prompt == "once" && shown) {
            return;
        }
        let folder = workspace.file.is_none();
        let target = if folder { "folder" } else { "workspace" };
        let label = match &workspace.file {
            None => path_label(&workspace.folders[0].path, None),
            Some(file) => format!("{} (Workspace)", path_label(&file.with_extension(""), None)),
        };
        // shortcut: no productService.nameShort " Dev" suffix, the port names itself as a release build does.
        let details = vec![
            format!("{NAME_LONG} provides features that may automatically execute files in this {target}."),
            "If you don't trust the authors of these files, we recommend to continue in restricted mode as the files may be malicious. See [our docs](https://aka.ms/vscode-workspace-trust) to learn more.".into(),
            format!("`{label}`"),
        ];
        let checkbox = self.workspace_trust.parent_folder().and_then(|parent| parent.file_name()).map(|name| format!("Trust the authors of all files in the parent folder '{}'", name.to_string_lossy()));
        let dialog = CustomDialog {
            icon: "shield",
            message: format!("Do you trust the authors of the files in this {target}?"),
            detail: None,
            details,
            checkbox,
            buttons: vec![
                DialogButton { label: "Yes, I trust the authors".into(), detail: Some(format!("Trust {target} and enable all features")) },
                DialogButton { label: "No, I don't trust the authors".into(), detail: Some(format!("Open {target} in restricted mode")) },
            ],
            cancel: None,
        };
        self.workspace_trust.startup_prompt_open = true;
        let answer = self.show_custom_dialog(dialog, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(DialogResult { button, checkbox_checked }) = answer.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.workspace_trust.startup_prompt_open = false;
                workbench.workspace_storage.lock().unwrap().set(STARTUP_PROMPT_SHOWN_KEY, "true".into());
                match (button, checkbox_checked) {
                    (0, true) => workbench.set_parent_folder_trust(window, cx),
                    (0, false) => workbench.complete_workspace_trust_request(window, cx),
                    _ => workbench.resolve_workspace_trust_requests(Value::Null),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// mainThreadWorkspace.ts `$requestWorkspaceTrust`: WorkspaceTrustRequestHandler's modal dialog. Callers
    /// arriving while a request is open share its answer.
    pub(crate) fn request_workspace_trust(&mut self, options: &Value, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.workspace_trust.is_trusted() {
            return Outcome::Ready(Ok(json!(true)));
        }
        let (sender, receiver) = oneshot::channel();
        self.workspace_trust.requests.push(sender);
        if self.workspace_trust.requests.len() > 1 || self.workspace_trust.startup_prompt_open {
            return Outcome::Pending(receiver);
        }
        let workspace_language = self.workspace().is_none_or(|workspace| workspace.file.is_some());
        let (message, trust_label) = match workspace_language {
            true => ("Do you trust the authors of the files in this workspace?", "Trust Workspace & Continue"),
            false => ("Do you trust the authors of the files in this folder?", "Trust Folder & Continue"),
        };
        let details = options["message"].as_str().unwrap_or("A feature you are trying to use may be a security risk if you do not trust the source of the files or folders you currently have open.");
        let dialog = CustomDialog {
            icon: "shield",
            message: message.into(),
            detail: None,
            details: vec![
                details.into(),
                "If you don't trust the authors of these files, we do not recommend continuing as the files may be malicious. See [our docs](https://aka.ms/vscode-workspace-trust) to learn more.".into(),
            ],
            checkbox: None,
            buttons: vec![
                DialogButton { label: trust_label.into(), detail: None },
                DialogButton { label: "Manage".into(), detail: None },
                DialogButton { label: "Cancel".into(), detail: None },
            ],
            cancel: Some(2),
        };
        let answer = self.show_custom_dialog(dialog, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(DialogResult { button, .. }) = answer.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| match button {
                0 => workbench.complete_workspace_trust_request(window, cx),
                1 => {
                    workbench.resolve_workspace_trust_requests(Value::Null);
                    workbench.execute_command(MANAGE_TRUST_COMMAND_ID, Vec::new(), window, cx);
                }
                _ => workbench.resolve_workspace_trust_requests(Value::Null),
            });
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// mainThreadWorkspace.ts `$isResourceTrusted`: `getUriTrustInfo`, which knows `file` folders only.
    pub(crate) fn is_resource_trusted(&self, uri: &Value) -> Outcome {
        if !self.workspace_trust.enabled {
            return Outcome::Ready(Ok(json!(true)));
        }
        Outcome::Ready(uri_parts(uri).map(|(scheme, path)| json!(scheme == "file" && self.workspace_trust.uri_trust_info(&path).0)))
    }

    /// `completeWorkspaceTrustRequest(true)`.
    fn complete_workspace_trust_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty_window_trusted = self.setting("security.workspace.trust.emptyWindow") == true;
        let changed = self.workspace_trust.set_workspace_trust(true, &mut self.workspace_storage.lock().unwrap(), empty_window_trusted);
        self.resolve_workspace_trust_requests(json!(true));
        if changed {
            self.workspace_trust_changed(window, cx);
        }
    }

    fn set_parent_folder_trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty_window_trusted = self.setting("security.workspace.trust.emptyWindow") == true;
        let changed = self.workspace_trust.set_parent_folder_trust(true, &mut self.workspace_storage.lock().unwrap(), empty_window_trusted);
        self.resolve_workspace_trust_requests(json!(self.workspace_trust.is_trusted()));
        if changed {
            self.workspace_trust_changed(window, cx);
        }
    }

    fn resolve_workspace_trust_requests(&mut self, answer: Value) {
        for request in self.workspace_trust.requests.drain(..) {
            let _ = request.send(Ok(answer.clone()));
        }
    }

    /// `onDidChangeTrust`. shortcut: the port's extension host cannot add or remove extensions while it runs
    /// (`$deltaExtensions`), so a trust change reloads the window instead of enabling the restricted extensions in place.
    fn workspace_trust_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.execute_command("workbench.action.reloadWindow", Vec::new(), window, cx);
    }

    /// `WorkbenchState`'s word in the Restricted Mode texts: window, folder or workspace.
    fn trust_target(&self) -> &'static str {
        match self.workspace() {
            None => "window",
            Some(workspace) if workspace.file.is_some() => "workspace",
            Some(_) => "folder",
        }
    }

    /// `getRestrictedModeStatusbarEntry`, added at `status.host` with priority Infinity: right after the remote
    /// indicator.
    pub(super) fn restricted_mode_status_item(&self, foreground: Rgba, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.workspace_trust.is_trusted() {
            return None;
        }
        let target = self.trust_target();
        let entry = StatusEntry {
            entry_id: "status.workspaceTrust".into(),
            id: "status.workspaceTrust".into(),
            name: "Workspace Trust".into(),
            text: "$(shield) Restricted Mode".into(),
            tooltip: None,
            command: Some(json!(MANAGE_TRUST_COMMAND_ID)),
            color: None,
            background_color: None,
            kind: Some("prominent"),
            align_left: true,
            priority: f64::INFINITY,
            secondary: 0,
            show_progress: false,
        };
        let tooltip = format!(
            "Running in Restricted Mode\n\nSome [features are disabled](command:{LIST_UNSUPPORTED_EXTENSIONS_COMMAND_ID}) because this [{target} is not trusted](command:{MANAGE_TRUST_COMMAND_ID})."
        );
        let item = self.extension_status_item(&entry, foreground, cx);
        Some(item.child(self.hover_target("status.workspaceTrust", HoverContent::Markdown(tooltip), HoverOptions::status_bar())))
    }

    /// `getBannerItem` shown by bannerPart.ts: 26px under the title bar while the window is untrusted, unless
    /// `security.workspace.trust.banner` hides it or it was closed with `untilDismissed`.
    pub(super) fn restricted_mode_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.workspace_trust.is_trusted() || self.workspace_trust.startup_prompt_open {
            return None;
        }
        let setting = self.setting("security.workspace.trust.banner");
        let dismissed = self.workspace_storage.lock().unwrap().get(BANNER_DISMISSED_KEY) == Some("true");
        if setting == "never" || (setting == "untilDismissed" && dismissed) {
            return None;
        }
        let foreground = self.color("banner.foreground");
        let message = format!("Restricted Mode is intended for safe code browsing. Trust this {} to enable all features.", self.trust_target());
        let action = |id: &'static str, label: &'static str, href: &'static str| {
            div()
                .id(id)
                .ml(px(12.))
                .p(px(3.))
                .underline()
                .cursor_pointer()
                .child(label)
                .on_click(cx.listener(move |workbench, _, window, cx| workbench.open_notification_link(href, window, cx)))
        };
        let close = self
            .icon("close", 22., 16., foreground)
            .id("banner.close")
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|workbench, _: &ClickEvent, _, cx| workbench.close_restricted_mode_banner(cx)))
            .child(self.hover_target("banner.close", HoverContent::Text("Close Banner".into()), HoverOptions::mouse()));
        Some(
            div()
                .h(px(26.))
                .flex_none()
                .flex()
                .bg(self.color("banner.background"))
                .text_color(foreground)
                .text_size(px(12.))
                .line_height(px(26.))
                .child(div().flex().flex_none().items_center().pl(px(10.)).pr(px(6.)).child(self.icon("shield", 16., 16., self.color("banner.iconForeground"))))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(message))
                .child(
                    div()
                        .flex_grow(1.)
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .child(action("banner.manage", "Manage", "command:workbench.trust.manage"))
                        .child(action("banner.learnMore", "Learn More", LEARN_MORE_URL)),
                )
                .child(div().flex().items_center().pl(px(6.)).pr(px(10.)).child(close)),
        )
    }

    /// The banner's `onClose`: dismissed for this workspace in Restricted Mode.
    fn close_restricted_mode_banner(&mut self, cx: &mut Context<Self>) {
        self.workspace_storage.lock().unwrap().set(BANNER_DISMISSED_KEY, "true".into());
        cx.notify();
    }
}
