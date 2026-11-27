//! workspaceTrustEditor.ts `WorkspaceTrustEditor` (`workbench.trust.manage`): the trust header with its Trust or
//! Don't Trust button, what a trusted folder and Restricted Mode allow, and the trusted folders table.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, ScrollHandle, Stateful, Subscription, Window, div, prelude::*, px};
use serde_json::Value;

use super::Workbench;
use super::hover::{HoverContent, HoverOptions};
use super::input_box_view::{InputId, InputLook, InputView};
use super::keybindings_editor_view::ModalHeader;
use crate::commands::Outcome;
use crate::file_dialog::FileDialog;
use crate::input_box::InputBox;
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
    /// `navOrder`: the header, the trusted and Restricted Mode boxes and the trusted folders section.
    sections: Vec<FocusHandle>,
    /// Edit Path: the row being edited and its path input.
    pub edit: Option<(PathBuf, InputBox)>,
    pub path_view: InputView,
    pub path_focus: Option<FocusHandle>,
    _path_blur: Option<Subscription>,
}

/// The `navOrder` sections, in their order.
const HEADER: usize = 0;
const TRUSTED: usize = 1;
const UNTRUSTED: usize = 2;
const CONFIGURATION: usize = 3;

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
        if self.trust_editor.sections.is_empty() {
            self.trust_editor.sections = (0..4).map(|_| cx.focus_handle()).collect();
        }
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
            .child(self.trust_section(HEADER, self.trust_header(trusted, cx), window))
            .child(
                div()
                    .id("workspace-trust-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.trust_editor.scroll)
                    .child(self.trust_features(trusted, window, cx))
                    .child(self.trust_section(CONFIGURATION, self.trusted_folders(window, cx), window)),
            )
    }

    /// The root's keydown: Up and Down focus the previous or next section (`navOrder`, wrapping), Escape the editor,
    /// `KeyMod.CtrlCmd | KeyCode.Enter` toggles the trust when it can be set.
    fn workspace_trust_editor_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let plain = !modifiers.platform && !modifiers.shift && !modifiers.alt && !modifiers.control;
        match keystroke.key.as_str() {
            "up" | "down" if plain => {
                let sections = self.trust_editor.sections.clone();
                let current = sections.iter().position(|section| section.contains_focused(window, cx)).map_or(-1, |index| index as isize);
                let next = match keystroke.key.as_str() {
                    "down" => current + 1,
                    _ => current.max(0) - 1,
                };
                let next = (next + sections.len() as isize) as usize % sections.len();
                window.focus(&sections[next], cx);
                // The browser scrolls a focused element into view; the header sits above the scrolled body.
                match next {
                    TRUSTED | UNTRUSTED => self.trust_editor.scroll.scroll_to_item(0),
                    CONFIGURATION => self.trust_editor.scroll.scroll_to_item(1),
                    _ => {}
                }
                cx.notify();
            }
            "escape" if plain => {
                let focus = self.trust_editor.focus.clone().expect("the editor is open");
                window.focus(&focus, cx);
                cx.notify();
            }
            "enter" if modifiers.platform && !modifiers.shift && !modifiers.alt && !modifiers.control => {
                cx.stop_propagation();
                let trusted = self.workspace_trust.is_trusted();
                self.set_workspace_trust(!trusted, window, cx);
            }
            _ => {}
        }
    }

    /// A `tabIndex = 0` section: focusable, outlined in focusBorder while focused (style.css `[tabindex="0"]:focus`).
    fn trust_section(&self, index: usize, content: Div, window: &Window) -> Div {
        let focus = &self.trust_editor.sections[index];
        let outline = self.theme.color("focusBorder").filter(|_| focus.is_focused(window));
        content.track_focus(focus).relative().when_some(outline, |section, color| section.child(div().absolute().inset_0().border_1().border_color(color)))
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

        let mut trusted_box = self.trust_section(TRUSTED, self.trust_limitations(trusted, trusted_title, trusted_subtitle), window).child(self.trust_list("trusted", &trusted_items, "check", self.color("debugIcon.startForeground"), cx));
        let mut untrusted_box = self.trust_section(UNTRUSTED, self.trust_limitations(!trusted, "In Restricted Mode", &untrusted_subtitle), window).child(self.trust_list("untrusted", &untrusted_items, "close", self.color("editorError.foreground"), cx));
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
    fn trusted_folders(&self, window: &Window, cx: &mut Context<Self>) -> Div {
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
            table = table.child(self.trusted_folder_row(index, path, *related, window, cx));
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

    fn trusted_folder_row(&self, index: usize, path: &Path, related: bool, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let group: gpui::SharedString = format!("trusted-folder-{index}").into();
        let selected = self.trust_editor.selected.as_deref() == Some(path);
        let label = path.to_string_lossy().into_owned();
        let picked = path.to_owned();
        let deleted = path.to_owned();
        let clicked = path.to_owned();
        let opened = path.to_owned();
        let edited = path.to_owned();
        let editing = self.trust_editor.edit.as_ref().is_some_and(|(row, _)| row == path);
        let actions = div()
            .w(px(75.))
            .pr(px(3.))
            .flex()
            .justify_end()
            .child(self.trust_action_button(format!("trust-pick-{index}"), "folder", "Open File Picker", cx, move |workbench, window, cx| workbench.pick_trusted_folder(picked.clone(), window, cx)))
            .child(self.trust_action_button(format!("trust-edit-{index}"), "edit", "Edit Path", cx, move |workbench, window, cx| workbench.edit_trusted_folder(edited.clone(), window, cx)))
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
            .child(match editing {
                // `.path.input-mode`: the input replaces the label and the cell loses its 5px padding.
                true => self.trusted_path_input(window, cx).into_any_element(),
                false => div().flex_1().min_w_0().pl(px(5.)).truncate().child(label).into_any_element(),
            })
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
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.replace_trusted_folder(path, new_path, window, cx));
        })
        .detach();
    }

    fn trusted_path_input(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.trust_editor.path_focus.as_ref().is_some_and(|focus| focus.is_focused(window));
        let border = if focused { self.theme.color("focusBorder") } else { self.theme.color("input.border") };
        div()
            .flex_1()
            .min_w_0()
            .h(px(ROW_HEIGHT))
            .flex()
            .bg(self.color("input.background"))
            .text_color(self.foreground_or("input.foreground"))
            .when_some(border, |input, color| input.border_1().border_color(color))
            .child(div().flex_1().h_full().px(px(4.)).child(self.input_text(InputId::TrustedPath, InputLook { password: false, placeholder: None, focused }, cx)))
    }

    /// `edit(item, false)`: the row's path in an input, focused and selected; Enter accepts, Escape or blur rejects.
    fn edit_trusted_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = InputBox::new(path.to_string_lossy().into_owned());
        input.select_all();
        self.trust_editor.selected = Some(path.clone());
        self.trust_editor.edit = Some((path, input));
        let focus = self.trust_editor.path_focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        self.trust_editor._path_blur = Some(cx.on_blur(&focus, window, |workbench, _, cx| drop(workbench.end_trusted_path_edit(cx))));
        self.blink_caret(InputId::TrustedPath, window, cx);
        cx.notify();
    }

    pub(super) fn edit_trusted_path_input(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        if let Some((_, input)) = self.trust_editor.edit.as_mut() {
            edit(input);
        }
        cx.notify();
    }

    /// The path input's Enter (`acceptEdit`: the new path replaces the row's) and Escape (`rejectEdit`).
    pub(super) fn trusted_path_input_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match key {
            "enter" => {
                let Some((path, input)) = self.end_trusted_path_edit(cx) else { return true };
                self.replace_trusted_folder(path, PathBuf::from(input.value), window, cx);
            }
            "escape" => {
                self.end_trusted_path_edit(cx);
            }
            _ => return false,
        }
        let focus = self.trust_editor.focus.clone().expect("the editor is open");
        window.focus(&focus, cx);
        true
    }

    fn end_trusted_path_edit(&mut self, cx: &mut Context<Self>) -> Option<(PathBuf, InputBox)> {
        self.trust_editor._path_blur = None;
        self.trust_editor.path_view.reset();
        cx.notify();
        self.trust_editor.edit.take()
    }

    /// `acceptEdit`: the new folder takes the old one's place among the trusted folders.
    fn replace_trusted_folder(&mut self, old: PathBuf, new: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.set_trusted_folders(std::slice::from_ref(&new), true, window, cx);
        if new != old {
            self.set_trusted_folders(&[old], false, window, cx);
        }
        self.trust_editor.selected = Some(new);
        cx.notify();
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
fn trust_editor_window<'a>(name: &str, cx: &'a mut gpui::TestAppContext) -> (PathBuf, gpui::Entity<Workbench>, &'a mut gpui::VisualTestContext) {
    let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
    let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
    let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
    let dir = std::env::temp_dir().join(format!("code-gpui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("old")).unwrap();
    let host = crate::host_sync::HostSync::new(std::sync::mpsc::channel().0);
    let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), crate::default_settings(), host, cx));
    workbench.update_in(cx, |workbench, window, cx| {
        workbench.set_trusted_folders(&[dir.join("old")], true, window, cx);
        drop(workbench.execute_command(super::workspace_trust_view::MANAGE_TRUST_COMMAND_ID, Vec::new(), window, cx));
    });
    cx.run_until_parked();
    (dir, workbench, cx)
}

/// workspaceTrustEditor.ts `TrustedUriPathColumnRenderer`: Edit Path puts the path in a selected input; Enter
/// trusts the typed path in place of the row's, Escape keeps the row.
#[cfg(test)]
#[gpui::test]
fn edit_path_replaces_the_trusted_folder_on_enter_and_keeps_it_on_escape(cx: &mut gpui::TestAppContext) {
    let (dir, workbench, cx) = trust_editor_window("trust-edit-path", cx);
    let trusted = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.workspace_trust.trusted_paths().collect::<Vec<_>>());
    let edit = |cx: &mut gpui::VisualTestContext, path: PathBuf| workbench.update_in(cx, |workbench, window, cx| workbench.edit_trusted_folder(path, window, cx));
    edit(cx, dir.join("old"));
    cx.simulate_input(&dir.join("new").to_string_lossy());
    cx.simulate_keystrokes("escape");
    assert_eq!(trusted(cx), [dir.join("old")]);
    assert!(workbench.read_with(cx, |workbench, _| workbench.trust_editor.edit.is_none()));

    edit(cx, dir.join("old"));
    cx.simulate_input(&dir.join("new").to_string_lossy());
    cx.simulate_keystrokes("enter");
    assert_eq!(trusted(cx), [dir.join("new")]);
    assert_eq!(workbench.read_with(cx, |workbench, _| workbench.trust_editor.selected.clone()), Some(dir.join("new")));
    std::fs::remove_dir_all(dir).unwrap();
}

/// The root's keydown: Down from the editor focuses the header, Up from the header wraps to the trusted folders.
#[cfg(test)]
#[gpui::test]
fn up_and_down_move_the_focus_between_the_sections(cx: &mut gpui::TestAppContext) {
    let (dir, workbench, cx) = trust_editor_window("trust-sections", cx);
    let focused = |cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, _| workbench.trust_editor.sections.iter().position(|section| section.is_focused(window)));
    cx.simulate_keystrokes("down");
    assert_eq!(focused(cx), Some(HEADER));
    cx.simulate_keystrokes("down down");
    assert_eq!(focused(cx), Some(UNTRUSTED));
    cx.simulate_keystrokes("up up up");
    assert_eq!(focused(cx), Some(CONFIGURATION));
    cx.simulate_keystrokes("escape");
    assert_eq!(focused(cx), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(test)]
#[test]
fn trusted_entries_sort_workspace_files_last_and_mark_the_parents_of_the_window() {
    let paths = vec![PathBuf::from("/b.code-workspace"), PathBuf::from("/z"), PathBuf::from("/a")];
    let entries = trusted_entries(paths, &[PathBuf::from("/z/project")]);
    assert_eq!(entries, [(PathBuf::from("/a"), false), (PathBuf::from("/z"), true), (PathBuf::from("/b.code-workspace"), false)]);
}
