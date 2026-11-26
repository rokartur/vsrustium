//! workspaceTrustEditor.ts `WorkspaceTrustEditor` (`workbench.trust.manage`): the trust header with its Trust or
//! Don't Trust button, what a trusted folder and Restricted Mode allow, and the trusted folders table.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, ScrollHandle, Stateful, Window, div, prelude::*, px};
use serde_json::Value;

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::keybindings_editor_view::ModalHeader;
use crate::commands::Outcome;
use crate::file_dialog::FileDialog;
use crate::walkthroughs::{Node, parse_linked_text};

/// `TrustedUriTableVirtualDelegate`.
const HEADER_ROW_HEIGHT: f32 = 30.;
const ROW_HEIGHT: f32 = 24.;

#[derive(Default)]
pub struct TrustEditor {
    open: bool,
    /// The editor's own focus, inside the modal part; the part's handle only places it (key handlers sit below it).
    focus: Option<FocusHandle>,
    part_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    /// The selected row of the trusted folders table.
    selected: Option<PathBuf>,
}

impl Workbench {
    pub(super) fn register_workspace_trust_editor_commands(&mut self) {
        self.commands.register(super::workspace_trust_view::MANAGE_TRUST_COMMAND_ID, |workbench, _, window, cx| {
            workbench.open_workspace_trust_editor(window, cx);
            Outcome::done()
        });
    }

    /// workspace.contribution.ts `ManageWorkspaceTrustAction`; the input is `RequiresModal`, so it opens in the modal
    /// editor part as the Settings and Profiles editors do.
    fn open_workspace_trust_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal_editor(window, cx);
        self.trust_editor.open = true;
        let focus = self.trust_editor.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn close_workspace_trust_editor(&mut self, cx: &mut Context<Self>) {
        self.trust_editor.open = false;
        cx.notify();
    }

    pub(super) fn workspace_trust_editor_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.trust_editor.open {
            return None;
        }
        let focus = self.trust_editor.focus.clone()?;
        let part_focus = self.trust_editor.part_focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let header = ModalHeader { icon: "shield", title: "Workspace Trust", json_title: "", json_command: "", resource: None };
        Some(self.modal_editor_part(header, &part_focus, window, cx, |workbench, _, _, window, cx| workbench.workspace_trust_editor_body(&focus, window, cx).into_any_element()))
    }

    fn workspace_trust_editor_body(&self, focus: &FocusHandle, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let trusted = self.workspace_trust.is_trusted();
        div()
            .id("workspace-trust-editor")
            .track_focus(focus)
            .key_context("WorkspaceTrustEditor")
            .on_key_down(cx.listener(|workbench, event: &KeyDownEvent, window, cx| workbench.workspace_trust_editor_key(event, window, cx)))
            .flex_1()
            .min_h_0()
            .max_w(px(1600.))
            .pt(px(12.))
            .flex()
            .flex_col()
            .child(self.trust_header(trusted, cx))
            .child(
                div()
                    .id("workspace-trust-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.trust_editor.scroll)
                    .child(self.trust_features(trusted, window, cx))
                    .child(self.trusted_folders(cx)),
            )
    }

    /// `KeyMod.CtrlCmd | KeyCode.Enter` toggles the trust when it can be set.
    fn workspace_trust_editor_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "enter" && keystroke.modifiers.platform && !keystroke.modifiers.shift && !keystroke.modifiers.alt && !keystroke.modifiers.control {
            cx.stop_propagation();
            let trusted = self.workspace_trust.is_trusted();
            self.set_workspace_trust(!trusted, window, cx);
        }
    }

    fn trust_header(&self, trusted: bool, cx: &mut Context<Self>) -> Div {
        let title = match (trusted, self.trust_target()) {
            (false, _) => "You are in Restricted Mode",
            (true, "window") => "You trust this window",
            (true, "folder") => "You trust this folder",
            (true, _) => "You trust this workspace",
        };
        let description = match trusted {
            true => "All features are enabled because trust has been granted to the workspace.".to_owned(),
            false => format!("{} is in a restricted mode intended for safe code browsing.", crate::product::get("nameShort").as_str().unwrap_or_default()),
        };
        let actions = "[Configure your settings](command:workbench.trust.configure) or [learn more](https://aka.ms/vscode-workspace-trust).";
        div()
            .px(px(24.))
            .py(px(16.))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .py(px(10.))
                    .flex()
                    .items_center()
                    .text_size(px(24.))
                    .line_height(px(32.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.icon("shield", 32., 32., self.color("button.background")).mr(px(8.)))
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(600.))
                    .py(px(8.))
                    .line_height(px(20.))
                    .text_center()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(description)
                    .child(self.trust_linked_text("trust-header", actions, cx)),
            )
    }

    /// `renderAffectedFeatures`: the trusted and the Restricted Mode boxes side by side, the window's own outlined.
    fn trust_features(&self, trusted: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let target = self.trust_target();
        let empty = target == "window";
        let (trusted_title, trusted_subtitle) = match target {
            "window" => ("In a Trusted Window", "You trust the authors of the files in the current window. All features are enabled:"),
            "folder" => ("In a Trusted Folder", "You trust the authors of the files in the current folder. All features are enabled:"),
            _ => ("In a Trusted Workspace", "You trust the authors of the files in the current workspace. All features are enabled:"),
        };
        let untrusted_subtitle = format!("You do not trust the authors of the files in the current {target}. The following features are disabled:");
        let mut trusted_items = vec!["Tasks are allowed to run".to_owned(), "Debugging is enabled".to_owned()];
        let extensions = format!("[{} extensions](command:workbench.extensions.action.listWorkspaceUnsupportedExtensions) are disabled or have limited functionality", self.trust_limited_extension_count());
        let mut untrusted_items = vec!["Tasks are not allowed to run".to_owned(), "Debugging is disabled".to_owned()];
        if !empty {
            trusted_items.push("All workspace settings are applied".to_owned());
            untrusted_items.push(match self.restricted_settings_count() {
                0 => "Workspace settings requiring trust are not applied".to_owned(),
                count => format!("[{count} workspace settings](command:settings.filterUntrusted) are not applied"),
            });
        }
        trusted_items.push("All enabled extensions are activated".to_owned());
        untrusted_items.push(extensions);

        let mut trusted_box = self.trust_limitations(trusted, trusted_title, trusted_subtitle).child(self.trust_list("trusted", &trusted_items, "check", self.color("debugIcon.startForeground"), cx));
        let mut untrusted_box = self.trust_limitations(!trusted, "In Restricted Mode", &untrusted_subtitle).child(self.trust_list("untrusted", &untrusted_items, "close", self.color("editorError.foreground"), cx));
        match trusted {
            true => untrusted_box = untrusted_box.child(self.trust_button("trust-deny", "Don't Trust", false, window, cx)),
            false => trusted_box = trusted_box.child(self.trust_button("trust-grant", "Trust", true, window, cx)),
        }
        div().p(px(24.)).flex().flex_wrap().justify_center().gap(px(16.)).child(trusted_box).child(untrusted_box)
    }

    /// `.workspace-trust-limitations`: the box, outlined in the button color when it is the window's state.
    fn trust_limitations(&self, current: bool, title: &str, subtitle: &str) -> Div {
        let selected = self.color("button.background");
        div()
            .relative()
            .flex_1()
            .min_w(px(250.))
            .max_w(px(400.))
            .px(px(36.))
            .py(px(8.))
            .flex()
            .flex_col()
            .rounded(px(6.))
            .border_1()
            .border_color(self.color("button.secondaryBackground"))
            .when(current, |limitations| limitations.child(div().absolute().inset_0().rounded(px(6.)).border_2().border_color(selected)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .py(px(16.))
                            .flex()
                            .items_center()
                            .text_size(px(16.))
                            .line_height(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.to_owned()),
                    )
                    .child(div().w_full().line_height(px(18.)).child(subtitle.to_owned())),
            )
    }

    fn trust_list(&self, id: &str, items: &[String], icon: &str, color: gpui::Rgba, cx: &mut Context<Self>) -> Div {
        let mut list = div().my(px(16.)).flex().flex_col();
        for (index, item) in items.iter().enumerate() {
            list = list.child(
                div()
                    .pb(px(4.))
                    .flex()
                    .child(self.icon(icon, 16., 16., color).h(px(24.)).mr(px(6.)))
                    .child(self.trust_linked_text(&format!("{id}-{index}"), item, cx).text_size(px(14.)).line_height(px(24.))),
            );
        }
        list
    }

    /// `createButtonRow` with a `ButtonWithDescription`: the label over the Cmd+Enter keybinding.
    fn trust_button(&self, id: &'static str, label: &'static str, trust: bool, _window: &Window, cx: &mut Context<Self>) -> Div {
        // Measured against the reference with ui-pair (m12/trust-editor.json): a 77x25 label box over the italic keybinding.
        let button = div()
            .mx(px(4.))
            .my(px(5.))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .id(id)
                    .min_w(px(77.))
                    .px(px(10.))
                    .h(px(25.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(self.color("button.background"))
                    .text_color(self.color("button.foreground"))
                    .cursor_pointer()
                    .hover(|button| button.bg(self.color("button.hoverBackground")))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.set_workspace_trust(trust, window, cx)))
                    .child(label),
            )
            .child(div().px(px(20.)).py(px(4.)).text_size(px(11.)).italic().child("\u{2318}Enter"));
        div().mt_auto().pt(px(5.)).pb(px(10.)).flex().items_center().justify_center().child(button)
    }

    /// Text and `[label](target)` links, opened as the editor's `Link` widgets are.
    fn trust_linked_text(&self, id: &str, text: &str, cx: &mut Context<Self>) -> Div {
        let mut paragraph = div().flex().flex_wrap();
        for (index, node) in parse_linked_text(text).into_iter().enumerate() {
            match node {
                Node::Text(text) => paragraph = paragraph.children(text.split_inclusive(' ').map(str::to_owned).collect::<Vec<_>>()),
                Node::Link { label, href, .. } => {
                    paragraph = paragraph.child(
                        div()
                            .id(gpui::SharedString::from(format!("{id}-link-{index}")))
                            .text_color(self.color("textLink.foreground"))
                            .cursor_pointer()
                            .hover(|link| link.text_color(self.color("textLink.activeForeground")).underline())
                            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.open_markdown_link(&href, window, cx)))
                            .child(label),
                    );
                }
            }
        }
        paragraph
    }

    /// `WorkspaceTrustedUrisTable`: the trusted `file` folders, each with Edit Path, Open File Picker and Delete Path
    /// on hover or selection, and Add Folder below.
    fn trusted_folders(&self, cx: &mut Context<Self>) -> Div {
        let workspace_uris: Vec<PathBuf> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).chain(workspace.file.clone()).collect()).unwrap_or_default();
        let entries = trusted_entries(self.workspace_trust.trusted_paths().collect(), &workspace_uris);
        let description = match entries.is_empty() {
            true => "You haven't trusted any folders or workspace files yet.",
            false => "You trust the following folders, their subfolders, and workspace files.",
        };
        let header_background = self.color("keybindingTable.headerBackground");
        let mut table = div().mt(px(16.)).flex().flex_col().child(
            div().h(px(HEADER_ROW_HEIGHT)).flex().items_center().bg(header_background).font_weight(FontWeight::SEMIBOLD).child(div().w_1_5().pl(px(5.)).child("Host")).child(div().flex_1().pl(px(5.)).child("Path")),
        );
        for (index, (path, related)) in entries.iter().enumerate() {
            table = table.child(self.trusted_folder_row(index, path, *related, cx));
        }
        div()
            .mt(px(8.))
            .px(px(36.))
            .py(px(24.))
            .border_t_1()
            .border_color(self.color("editorWidget.border"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Trusted Folders & Workspaces"))
            .child(div().child(description))
            .when(!entries.is_empty(), |settings| settings.child(table))
            .child(div().mt(px(8.)).flex().child(self.trust_text_button("trust-add-folder", "Add Folder", cx, |workbench, window, cx| workbench.add_trusted_folders(window, cx)).mx(px(4.)).my(px(8.))))
    }

    fn trusted_folder_row(&self, index: usize, path: &Path, related: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let group: gpui::SharedString = format!("trusted-folder-{index}").into();
        let selected = self.trust_editor.selected.as_deref() == Some(path);
        let label = path.to_string_lossy().into_owned();
        let picked = path.to_owned();
        let deleted = path.to_owned();
        let clicked = path.to_owned();
        let opened = path.to_owned();
        let actions = div()
            .w(px(75.))
            .pr(px(3.))
            .flex()
            .justify_end()
            .child(self.trust_action_button(format!("trust-pick-{index}"), "folder", "Open File Picker", cx, move |workbench, window, cx| workbench.pick_trusted_folder(picked.clone(), window, cx)))
            .child(self.trust_action_button(format!("trust-delete-{index}"), "close", "Delete Path", cx, move |workbench, window, cx| workbench.delete_trusted_folder(&deleted, window, cx)))
            .when(!selected, |actions| actions.invisible().group_hover(group.clone(), |actions| actions.visible()));
        div()
            .id(group.clone())
            .group(group)
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .when(related, |row| row.italic().font_weight(FontWeight::BOLD))
            .when(selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
            .hover(|row| row.bg(self.color("list.hoverBackground")))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                workbench.trust_editor.selected = Some(clicked.clone());
                // `table.onDidOpen` on a double click: the folder picker.
                if event.click_count() == 2 {
                    workbench.pick_trusted_folder(opened.clone(), window, cx);
                }
                cx.notify();
            }))
            .child(div().w_1_5().pl(px(5.)).truncate().child("Local"))
            .child(div().flex_1().min_w_0().pl(px(5.)).truncate().child(label))
            .child(actions)
    }

    fn trust_action_button(&self, id: String, icon: &str, tooltip: &'static str, cx: &mut Context<Self>, run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) -> Stateful<Div> {
        self.icon(icon, 20., 16., self.color("icon.foreground"))
            .id(gpui::SharedString::from(id.clone()))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("toolbar.hoverBackground")))
            .child(self.hover_target(gpui::SharedString::from(format!("{id}-hover")), HoverContent::Text(tooltip.into()), HoverOptions::mouse()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                run(workbench, window, cx);
            }))
    }

    fn trust_text_button(&self, id: &'static str, label: &'static str, cx: &mut Context<Self>, run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) -> Stateful<Div> {
        div()
            .id(id)
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .line_height(px(16.))
            .bg(self.color("button.background"))
            .text_color(self.color("button.foreground"))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("button.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| run(workbench, window, cx)))
            .child(label)
    }

    /// Add Folder: "Select Folders to Trust", trusted together (`setUrisTrust`), the topmost row selected.
    fn add_trusted_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_path = self.workspace().and_then(|workspace| workspace.folders.first()).map_or_else(|| PathBuf::from("/"), |folder| folder.path.clone());
        let dialog = FileDialog { title: Some("Select Folders to Trust".into()), button_label: Some("Trust Folders".into()), default_path: Some(default_path), folders: true, multiple: true, ..FileDialog::default() };
        let picked = self.show_open_dialog(dialog, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(paths) = picked.await else { return };
            if paths.is_empty() {
                return;
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.set_trusted_folders(&paths, true, window, cx);
                let added: Vec<PathBuf> = paths.iter().map(|path| workbench.workspace_trust.uri_trust_info(path).1).collect();
                let rows = trusted_entries(workbench.workspace_trust.trusted_paths().collect(), &[]);
                workbench.trust_editor.selected = rows.into_iter().map(|(row, _)| row).find(|row| added.contains(row));
                cx.notify();
            });
        })
        .detach();
    }

    /// `edit(item, true)`: the picked folder replaces the row's (`acceptEdit`).
    fn pick_trusted_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = FileDialog { title: Some("Select Folder To Trust".into()), button_label: Some("Trust Folder".into()), default_path: Some(path.clone()), folders: true, ..FileDialog::default() };
        let picked = self.show_open_dialog(dialog, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(paths) = picked.await else { return };
            let Some(new_path) = paths.into_iter().next() else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.set_trusted_folders(std::slice::from_ref(&new_path), true, window, cx);
                if new_path != path {
                    workbench.set_trusted_folders(&[path], false, window, cx);
                }
                workbench.trust_editor.selected = Some(new_path);
                cx.notify();
            });
        })
        .detach();
    }

    /// `delete`: the row after it (or before it) takes the selection.
    fn delete_trusted_folder(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let rows = trusted_entries(self.workspace_trust.trusted_paths().collect(), &[]);
        let index = rows.iter().position(|(row, _)| row == path);
        self.trust_editor.selected = index.and_then(|index| rows.get(index + 1).or_else(|| index.checked_sub(1).and_then(|before| rows.get(before)))).map(|(row, _)| row.clone());
        self.set_trusted_folders(&[path.to_owned()], false, window, cx);
        cx.notify();
    }

    /// The `restrictedSettings.default` count `render` shows: restricted settings a workspace can set, deprecated ones
    /// left out (port/notes.md milestone 12: upstream counts one the workspace sets).
    fn restricted_settings_count(&self) -> usize {
        let settings = self.settings.lock().unwrap();
        let registry = &settings.registry;
        registry
            .keys()
            .iter()
            .filter_map(|key| registry.property(key))
            .filter(|property| property.restricted && !config::registry::APPLICATION_SCOPES.contains(&property.scope) && property.scope != config::registry::Scope::Machine)
            .filter(|property| property.schema["deprecationMessage"].is_null() && property.schema["markdownDeprecationMessage"].is_null())
            .count()
    }

    /// `getExtensionCount`: enabled extensions that run code and do not fully support untrusted workspaces.
    fn trust_limited_extension_count(&self) -> usize {
        let configured = self.setting("extensions.supportUntrustedWorkspaces");
        self.extensions_viewlet
            .installed
            .iter()
            .filter(|extension| {
                let manifest = &extension.description;
                if manifest["main"].is_null() && manifest["browser"].is_null() {
                    return false;
                }
                let configured = configured.as_object().into_iter().flatten().find(|(id, _)| id.eq_ignore_ascii_case(&extension.id)).map(|(_, value)| value);
                let supported = match configured.filter(|configured| configured["version"].is_null() || configured["version"] == manifest["version"]) {
                    Some(configured) => &configured["supported"],
                    None => &manifest["capabilities"]["untrustedWorkspaces"]["supported"],
                };
                *supported != Value::Bool(true)
            })
            .count()
    }
}

/// `trustedUriEntries`: the trusted folders by path, workspace files last, each with whether it contains one of the
/// window's folders or its workspace file (drawn bold and italic).
fn trusted_entries(mut paths: Vec<PathBuf>, workspace_uris: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    paths.sort_by(|a, b| {
        let workspace_file = |path: &Path| path.extension().is_some_and(|extension| extension == "code-workspace");
        workspace_file(a).cmp(&workspace_file(b)).then_with(|| a.cmp(b))
    });
    paths
        .into_iter()
        .map(|path| {
            let related = workspace_uris.iter().any(|uri| uri.starts_with(&path));
            (path, related)
        })
        .collect()
}

#[cfg(test)]
#[test]
fn trusted_entries_sort_workspace_files_last_and_mark_the_parents_of_the_window() {
    let paths = vec![PathBuf::from("/b.code-workspace"), PathBuf::from("/z"), PathBuf::from("/a")];
    let entries = trusted_entries(paths, &[PathBuf::from("/z/project")]);
    assert_eq!(entries, [(PathBuf::from("/a"), false), (PathBuf::from("/z"), true), (PathBuf::from("/b.code-workspace"), false)]);
}
