//! The Extensions view as extensionsViewlet.ts and extensionsList.ts draw it (media/extensionsViewlet.css,
//! extension.css, extensionActions.css, extensionsWidgets.css): the search box with its clear and filter actions,
//! the views with their headers and count badges, and the 72px extension rows with icon, name, install count,
//! rating, description, publisher and action buttons.

use gpui::{AnyElement, ClickEvent, Context, Div, FontWeight, MouseButton, MouseDownEvent, ScrollStrategy, Stateful, Window, div, img, prelude::*, px, uniform_list};
use serde_json::{Value, json};

use super::Workbench;
use super::input_box_view::{InputId, InputLook};
use super::extensions_view::Pane;
use crate::extension_enablement::EnablementState;
use crate::extensions_model::{Extension, ExtensionState};
use crate::menus::MenuAction;

/// `EXTENSION_LIST_ELEMENT_HEIGHT`.
const ROW_HEIGHT: f32 = 72.;

/// An action button of a row or of the extension editor.
pub(super) struct ButtonSpec {
    pub label: String,
    pub prominent: bool,
    pub command: &'static str,
    pub args: Vec<Value>,
    /// The dropdown of `ButtonWithDropDownExtensionAction`: its menu items.
    pub dropdown: Vec<MenuAction>,
}

impl Workbench {
    /// The Extensions view's title: "Extensions", or "Extensions: <view>" while one view shows.
    pub(super) fn extensions_title(&self) -> String {
        match self.extensions_viewlet.panes.as_slice() {
            [pane] => format!("Extensions: {}", pane.descriptor.title),
            _ => "Extensions".into(),
        }
    }

    /// The view's title actions: Refresh and the More Actions menu (extensions.contribution.ts `ViewContainerTitle`).
    pub(super) fn extensions_title_actions(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .gap(px(4.))
            .child(self.search_action("refresh", "workbench.extensions.action.refreshExtension", true, cx))
            .child(self.icon("ellipsis", 22., 16., self.foreground_or("icon.foreground")).id("extensions-more").rounded(px(4.)).cursor_pointer().hover(|action| action.bg(self.color("toolbar.hoverBackground"))).on_mouse_down(
                MouseButton::Left,
                cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let anchor = gpui::Bounds::new(event.position, gpui::size(px(0.), px(0.)));
                    workbench.show_context_menu_actions(workbench.extensions_more_actions(), &Default::default(), anchor, Vec::new(), window, cx);
                }),
            ))
    }

    /// The `ViewContainerTitle` menu of extensions.contribution.ts, groups in their order.
    fn extensions_more_actions(&self) -> Vec<MenuAction> {
        let command = |id: &str, title: &str| MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked: false, alt: None };
        let auto_update_on = self.auto_update_on();
        vec![
            command("workbench.extensions.action.checkForUpdates", "Check for Extension Updates"),
            match auto_update_on {
                true => command("workbench.extensions.action.disableAutoUpdate", "Disable Auto Update for All Extensions"),
                false => command("workbench.extensions.action.enableAutoUpdate", "Enable Auto Update for All Extensions"),
            },
            command("workbench.extensions.action.updateAllExtensions", "Update All Extensions"),
            MenuAction::Separator,
            command("workbench.extensions.action.enableAll", "Enable All Extensions"),
            command("workbench.extensions.action.disableAll", "Disable All Installed Extensions"),
            MenuAction::Separator,
            MenuAction::Submenu { title: "Views".into(), actions: Vec::new(), checked: false },
            MenuAction::Separator,
            command("workbench.extensions.action.installVSIX", "Install from VSIX..."),
        ]
        .into_iter()
        .filter(|action| !matches!(action, MenuAction::Submenu { actions, .. } if actions.is_empty()))
        .collect()
    }

    /// The viewlet's body: the search header and the views.
    pub(super) fn extensions_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let panes = &self.extensions_viewlet.panes;
        let list_focused = self.extensions_viewlet.list_focus.is_focused(window);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.extensions_search_header(window, cx))
            .child(
                div()
                    .id("extensions-panes")
                    .track_focus(&self.extensions_viewlet.list_focus)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .children(panes.iter().enumerate().map(|(index, pane)| self.extensions_pane(index, pane, panes.len() == 1, list_focused, cx))),
            )
    }

    /// extensionsViewlet.css `.header`: 41px, the 28px search box with its clear and filter actions inside.
    fn extensions_search_header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let viewlet = &self.extensions_viewlet;
        let focused = viewlet.search_focus.is_focused(window);
        let border = if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or(self.color("input.background")) };
        let placeholder = Some("Search Extensions in Marketplace".to_owned());
        let has_value = !viewlet.search.value.is_empty();
        let clear = self.search_action("clear-all", "workbench.extensions.action.clearExtensionsSearchResults", has_value, cx);
        let filter = self.icon("filter", 22., 16., self.foreground_or("icon.foreground")).id("extensions-filter").rounded(px(4.)).cursor_pointer().hover(|action| action.bg(self.color("toolbar.hoverBackground"))).on_mouse_down(
            MouseButton::Left,
            cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let anchor = gpui::Bounds::new(event.position, gpui::size(px(0.), px(0.)));
                workbench.show_context_menu_actions(extensions_filter_actions(), &Default::default(), anchor, Vec::new(), window, cx);
            }),
        );
        let input = div()
            .relative()
            .h(px(28.))
            .flex()
            .items_center()
            .pl(px(6.))
            .pr(px(2.))
            .border_1()
            .rounded(px(4.))
            .bg(self.color("input.background"))
            .border_color(border)
            .text_color(self.foreground_or("input.foreground"))
            .child(div().flex_1().min_w_0().h_full().flex().child(self.input_text(InputId::ExtensionsSearch, InputLook { password: false, placeholder, focused }, cx)))
            .child(div().flex().gap(px(2.)).child(clear).child(filter));
        // Modern UI padding.css: 4px 8px around the 28px box.
        div().h(px(41.)).flex_none().py(px(4.)).px(px(8.)).child(input)
    }

    /// A view: its header, then its list, its message or nothing. A single view merges with the container
    /// (`mergeViewWithContainerWhenSingleView`): no header, always expanded.
    fn extensions_pane(&self, index: usize, pane: &Pane, single: bool, list_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let expanded = pane.expanded || single;
        // ExtensionsListView `updateSize`: a flexible view without results takes no height.
        let flexible = matches!(pane.descriptor.id, "workbench.views.extensions.installed" | "extensions.recommendedList");
        let badge = (pane.descriptor.id != "workbench.views.extensions.popular").then(|| self.count_badge(pane.extensions.len()));
        let header = self
            .section_header(pane.descriptor.title, expanded, index > 0)
            .id(("extensions-pane-header", index))
            .child(div().flex_1())
            .children(badge)
            .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                let pane = &mut workbench.extensions_viewlet.panes[index];
                pane.expanded = !pane.expanded;
                let (id, expanded) = (pane.descriptor.id, pane.expanded);
                workbench.extensions_viewlet.expansion.insert(id, expanded);
                cx.notify();
            }));
        let body: Option<AnyElement> = match () {
            _ if !expanded => None,
            _ if !pane.extensions.is_empty() => Some(self.extensions_list(index, pane, list_focused, cx)),
            _ if flexible => None,
            _ if pane.loading => None,
            _ => pane.message.as_ref().map(|(_, message)| {
                div().flex_1().child(div().pt(px(5.)).pr(px(9.)).pb(px(5.)).pl(px(20.)).cursor_default().text_color(self.foreground_or("sideBar.foreground")).child(message.clone())).into_any_element()
            }),
        };
        let grows = body.is_some();
        div()
            .flex()
            .flex_col()
            .when(grows, |pane| pane.flex_1().min_h_0())
            .when(!single, |pane| pane.child(header))
            .children(body)
            .into_any_element()
    }

    /// countBadge.ts under Modern UI (padding.css, roundedCorners.css, fontRamp.css): 19px circle, 10px text.
    fn count_badge(&self, count: usize) -> Div {
        div()
            .ml(px(4.))
            .mr(px(6.))
            .min_w(px(19.))
            .h(px(19.))
            .px(px(6.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.))
            .bg(self.color("badge.background"))
            .text_color(self.color("badge.foreground"))
            .when_some(self.theme.color("contrastBorder"), |badge, border| badge.border_1().border_color(border))
            .child(count.to_string())
    }

    fn extensions_list(&self, index: usize, pane: &Pane, list_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = pane.extensions.len();
        let list = uniform_list(
            ("extensions-list", index),
            count,
            cx.processor(move |workbench, range: std::ops::Range<usize>, window, cx| {
                let end = range.end;
                let rows: Vec<AnyElement> = range.map(|row| workbench.extension_row(index, row, list_focused, cx)).collect();
                if end + 5 >= workbench.extensions_viewlet.panes[index].extensions.len() {
                    workbench.load_next_extensions_page(index, window, cx);
                }
                rows
            }),
        )
        .track_scroll(&pane.scroll)
        .size_full();
        div().flex_1().min_h_0().child(list).into_any_element()
    }

    /// extensionsList.ts `Renderer`: the row of one extension.
    fn extension_row(&mut self, pane_index: usize, row: usize, list_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let extension = self.extensions_viewlet.panes[pane_index].extensions[row].clone();
        let icon_url = extension.gallery.as_ref().and_then(|gallery| gallery.icon.as_ref()).map(|icon| icon.uri.clone());
        if let (None, Some(url)) = (extension_local_icon(&extension), &icon_url) {
            self.request_extension_icon(url, cx);
        }
        let key = (pane_index, row);
        let viewlet = &self.extensions_viewlet;
        let is_focused = viewlet.focused == Some(key);
        let is_selected = viewlet.selected == Some(key);
        let (selection, selection_foreground) = match list_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| list_focused && is_focused);
        let disabled = extension.state == ExtensionState::Installed && !extension.enablement.is_enabled();
        let description_color = if is_selected || self.theme.kind.is_high_contrast() { None } else { Some(self.color("descriptionForeground")) };
        let mut header = div().h(px(20.)).flex().items_center().pr(px(10.)).overflow_hidden().child(
            div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::SEMIBOLD).child(extension.display_name().to_owned()),
        );
        if extension.state == ExtensionState::Uninstalled {
            header = header.children(self.install_count_small(&extension)).children(self.rating_small(&extension));
        }
        let description = match extension.gallery.as_ref().filter(|gallery| !gallery.localized_languages.is_empty()) {
            Some(gallery) => gallery.localized_languages.iter().map(|name| capitalize(name)).collect::<Vec<_>>().join(", "),
            None => extension.description().to_owned(),
        };
        let description = div().pr(px(8.)).overflow_hidden().whitespace_nowrap().text_ellipsis().when_some(description_color, |text, color| text.text_color(color)).child(description);
        let publisher = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .when(!is_selected, |text| text.text_color(if disabled { self.color("disabledForeground") } else { self.color("descriptionForeground") }))
            .child(extension.publisher_display_name().to_owned());
        let footer = div().h(px(24.)).pt(px(2.)).pr(px(2.)).flex().items_center().justify_end().overflow_hidden().child(publisher).child(self.extension_actions(&extension, false, cx));
        let icon = self.extension_icon(&extension, 36.).mr(px(16.));
        let bookmark = self.pre_release_bookmark(&extension);
        let id = extension.identifier().id;
        let row_element = div()
            .id(("extension-row", pane_index * 100_000 + row))
            .cursor_pointer()
            .relative()
            .size_full()
            .pl(px(16.))
            .flex()
            .rounded(px(4.))
            .when(disabled && !is_selected, |row| row.text_color(self.color("disabledForeground")))
            .when(!is_focused && !is_selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .when(is_focused && !is_selected && list_focused, |row| row.when_some(self.theme.color("list.focusBackground"), |row, color| row.bg(color)))
            .when(is_selected, |row| row.bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)))
            .when_some(focus_outline, |row, color| row.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color)))
            .child(div().flex().items_center().child(icon.when(disabled, |icon| icon.opacity(0.5))))
            .child(div().flex_1().min_w_0().flex().flex_col().justify_center().overflow_hidden().child(header).child(description).child(footer))
            .children(bookmark.map(|bookmark| bookmark.absolute().top_0().left_0()))
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                workbench.extensions_viewlet.focused = Some(key);
                workbench.extensions_viewlet.selected = Some(key);
                window.focus(&workbench.extensions_viewlet.list_focus.clone(), cx);
                if let Some(extension) = workbench.extensions_viewlet.panes.get(key.0).and_then(|pane| pane.extensions.get(key.1)).cloned() {
                    workbench.open_extension_editor(extension, true, window, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                    workbench.extensions_viewlet.focused = Some(key);
                    window.focus(&workbench.extensions_viewlet.list_focus.clone(), cx);
                    workbench.show_manage_extension_menu(&id, event.position, window, cx);
                }),
            );
        div().w_full().h(px(ROW_HEIGHT)).px(px(4.)).flex().child(row_element).into_any_element()
    }

    /// `ExtensionIconWidget`: the installed icon, else the gallery's, else the default codicon.
    pub(super) fn extension_icon(&self, extension: &Extension, size: f32) -> Div {
        let frame = div().size(px(size)).flex_none().flex().items_center().justify_center();
        if let Some(path) = extension_local_icon(extension) {
            return frame.child(img(path).size(px(size)).rounded(px(2.)));
        }
        let url = extension.gallery.as_ref().and_then(|gallery| gallery.icon.as_ref()).map(|icon| icon.uri.as_str());
        if let Some(Some(image)) = url.and_then(|url| self.extensions_viewlet.icons.get(url)) {
            return frame.child(img(image.clone()).size(px(size)).rounded(px(2.)));
        }
        // extensionsWorkbenchService.ts `defaultIconUrl` for themes and languages, else the extensions codicon.
        let manifest = extension.local.as_ref().map(|local| &local.description);
        let default = manifest.and_then(|manifest| {
            let media = self.app_root.join("src/vs/workbench/contrib/extensions/browser/media");
            let contributes = &manifest["contributes"];
            match () {
                _ if !extension.is_builtin() => None,
                _ if contributes["themes"].as_array().is_some_and(|themes| !themes.is_empty()) => Some(media.join("theme-icon.png")),
                _ if contributes["grammars"].as_array().is_some_and(|grammars| !grammars.is_empty()) => Some(media.join("language-icon.svg")),
                _ => None,
            }
        });
        match default {
            Some(path) => frame.child(img(path).size(px(size))),
            None => frame.child(self.icon("extensions", size, size, self.foreground_or("icon.foreground"))),
        }
    }

    /// `PreReleaseBookmarkWidget`: a 16px corner mark with the versions codicon.
    fn pre_release_bookmark(&self, extension: &Extension) -> Option<Div> {
        let shown = match extension.state {
            ExtensionState::Installed => extension.pre_release(),
            _ => extension.gallery.as_ref().is_some_and(|gallery| gallery.has_pre_release_version),
        };
        if !shown {
            return None;
        }
        let high_contrast = self.theme.kind.is_high_contrast();
        let (background, foreground) = match high_contrast {
            true => (self.color("editor.background"), self.color("contrastBorder")),
            false => (self.color("extensionIcon.preReleaseForeground"), gpui::rgb(0xffffff)),
        };
        Some(
            div()
                .size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_tl(px(4.))
                .rounded_br(px(4.))
                .bg(background)
                .when(high_contrast, |mark| mark.border_1().border_color(foreground))
                .child(self.icon("versions", 12., 12., foreground)),
        )
    }

    /// `InstallCountWidget` small: the cloud-download codicon and 9.4M, 36K or the count.
    fn install_count_small(&self, extension: &Extension) -> Option<Div> {
        let count = extension.install_count().filter(|count| *count > 0.)?;
        let label = match count {
            count if count > 1_000_000. => format!("{}M", (count / 100_000.).floor() / 10.),
            count if count > 1000. => format!("{}K", (count / 1000.).floor()),
            count => format!("{count}"),
        };
        let foreground = self.foreground_or("sideBar.foreground");
        Some(div().ml_auto().mr(px(6.)).flex().items_center().text_size(px(11.)).child(self.icon("cloud-download", 16., 16., foreground).mr(px(2.))).child(label))
    }

    /// `RatingsWidget` small: one star and the rating rounded to halves, when someone rated it.
    fn rating_small(&self, extension: &Extension) -> Option<Div> {
        let rating = extension.rating()?;
        if extension.rating_count().unwrap_or(0.) == 0. {
            return None;
        }
        let rating = (rating * 2.).round() / 2.;
        let star = self.color("extensionIcon.starForeground");
        Some(div().flex().items_center().text_size(px(10.4)).child(self.icon("star-full", 16., 16., star).mr(px(2.))).child(format!("{rating}")))
    }

    /// The row's or the editor's action bar: status labels, Update, the install dropdown, Installing, and the
    /// Manage gear (extensionsList.ts `actions`).
    pub(super) fn extension_actions(&self, extension: &Extension, in_editor: bool, cx: &mut Context<Self>) -> Div {
        let mut bar = div().flex().items_center().flex_none();
        for button in self.extension_buttons(extension, in_editor) {
            bar = bar.child(self.extension_button(button, 16., cx).ml(px(6.)));
        }
        if extension.state == ExtensionState::Installed && !in_editor {
            let id = extension.identifier().id;
            let gear = self
                .icon("gear", 20., 16., self.foreground_or("icon.foreground"))
                .id(gpui::SharedString::from(format!("manage-{id}")))
                .ml(px(6.))
                .mt(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|gear| gear.bg(self.color("toolbar.hoverBackground")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        workbench.show_manage_extension_menu(&id, event.position, window, cx);
                    }),
                );
            bar = bar.child(gear);
        }
        bar
    }

    /// The label buttons an extension shows in its current state, primary first.
    pub(super) fn extension_buttons(&self, extension: &Extension, in_editor: bool) -> Vec<ButtonSpec> {
        let id = extension.identifier().id;
        let button = |label: &str, prominent: bool, command: &'static str, args: Vec<Value>| ButtonSpec { label: label.into(), prominent, command, args, dropdown: Vec::new() };
        let mut buttons = Vec::new();
        match extension.state {
            ExtensionState::Uninstalled => {
                // InstallDropdownAction: with `preferPreReleases` (no `stable` quality) the pre-release install comes first.
                let has_pre_release = extension.gallery.as_ref().is_some_and(|gallery| gallery.has_pre_release_version);
                let mut install = button(if has_pre_release { "Install Pre-Release" } else { "Install" }, true, "extensions.install", vec![json!(id), json!(has_pre_release)]);
                if has_pre_release {
                    install.dropdown.push(MenuAction::Command { id: "extensions.install".into(), title: "Install Release Version".into(), enabled: true, checked: false, alt: None });
                }
                buttons.push(install);
            }
            ExtensionState::Installing => buttons.push(button("Installing", false, "", Vec::new())),
            ExtensionState::Uninstalling => buttons.push(button("Uninstalling", false, "", Vec::new())),
            ExtensionState::Installed => {
                if extension.restart_required && !extension.is_builtin() {
                    buttons.push(button("Restart Extensions", true, "workbench.action.restartExtensionHost", Vec::new()));
                }
                if extension.outdated() && extension.enablement.is_enabled() {
                    let label = if in_editor { format!("Update to v{}", extension.latest_version()) } else { "Update".into() };
                    buttons.push(ButtonSpec { label, prominent: true, command: "extensions.update", args: vec![json!(id)], dropdown: Vec::new() });
                }
                if in_editor {
                    buttons.extend(self.enablement_buttons(extension));
                    if !extension.is_builtin() {
                        buttons.push(button("Uninstall", false, "extensions.uninstall", vec![json!(id)]));
                    }
                }
            }
        }
        buttons
    }

    /// The editor's Enable and Disable dropdown buttons (`EnableDropDownAction`, `DisableDropDownAction`).
    fn enablement_buttons(&self, extension: &Extension) -> Vec<ButtonSpec> {
        let id = json!(extension.identifier().id);
        let has_workspace = self.folder.is_some();
        let command = |command: &str, title: &str| MenuAction::Command { id: command.into(), title: title.into(), enabled: true, checked: false, alt: None };
        match extension.enablement {
            EnablementState::DisabledGlobally | EnablementState::DisabledWorkspace => {
                let mut dropdown = Vec::new();
                if has_workspace {
                    dropdown.push(command("extensions.enableForWorkspace", "Enable (Workspace)"));
                }
                vec![ButtonSpec { label: "Enable".into(), prominent: false, command: "extensions.enableGlobally", args: vec![id], dropdown }]
            }
            EnablementState::EnabledGlobally | EnablementState::EnabledWorkspace if !extension.is_language_pack() => {
                let mut dropdown = Vec::new();
                if has_workspace {
                    dropdown.push(command("extensions.disableForWorkspace", "Disable (Workspace)"));
                }
                vec![ButtonSpec { label: "Disable".into(), prominent: false, command: "extensions.disableGlobally", args: vec![id], dropdown }]
            }
            _ => Vec::new(),
        }
    }

    /// actionbar.css `.action-label` 11px text; extensionActions.css `.extension-action.label` 5px padding, 14px line
    /// (16px in list rows, extension.css), extensionButton colors; a dropdown half after a separator.
    pub(super) fn extension_button(&self, button: ButtonSpec, line_height: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let (background, foreground, hover) = match button.prominent {
            true => (self.theme.color("extensionButton.prominentBackground"), self.foreground_or("extensionButton.prominentForeground"), self.theme.color("extensionButton.prominentHoverBackground")),
            false => (self.theme.color("extensionButton.background"), self.foreground_or("extensionButton.foreground"), self.theme.color("extensionButton.hoverBackground")),
        };
        let border = self.theme.color("extensionButton.border");
        let enabled = !button.command.is_empty();
        let has_dropdown = !button.dropdown.is_empty();
        let command = button.command;
        let args = button.args.clone();
        let label = div()
            .id(gpui::SharedString::from(format!("extension-button-{}-{}", button.label, args.first().and_then(Value::as_str).unwrap_or_default())))
            .px(px(5.))
            .text_size(px(11.))
            .line_height(px(line_height))
            .whitespace_nowrap()
            .max_w(px(150.))
            .overflow_hidden()
            .text_ellipsis()
            .when(enabled, |label| label.cursor_pointer().when_some(hover, |label, hover| label.hover(move |label| label.bg(hover))))
            .child(button.label.clone())
            .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                if !command.is_empty() {
                    drop(workbench.execute_command(command, args.clone(), window, cx));
                }
            }));
        let mut container = div()
            .id(gpui::SharedString::from(format!("extension-action-{}-{}", button.label, button.args.first().and_then(Value::as_str).unwrap_or_default())))
            .flex()
            .items_center()
            .rounded(px(4.))
            .overflow_hidden()
            .text_color(foreground)
            .when_some(background, |button, background| button.bg(background))
            .border_1()
            .border_color(border.unwrap_or(gpui::transparent_black().into()))
            .child(label);
        if has_dropdown {
            let separator = self.theme.color("extensionButton.separator").unwrap_or(foreground);
            let dropdown = button.dropdown;
            let args = button.args;
            let id = args.first().cloned().unwrap_or(Value::Null);
            container = container.child(div().w(px(1.)).h(px(10.)).bg(separator)).child(
                self.icon("chevron-down", 18., 16., foreground).id("extension-dropdown").cursor_pointer().when_some(hover, |icon, hover| icon.hover(move |icon| icon.bg(hover))).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let anchor = gpui::Bounds::new(event.position, gpui::size(px(0.), px(0.)));
                        workbench.show_context_menu_actions(dropdown.clone(), &Default::default(), anchor, vec![id.clone()], window, cx);
                    }),
                ),
            );
        }
        container
    }

    /// `ManageExtensionAction`: enablement, auto update, install and uninstall, then the extension context menu.
    pub(super) fn show_manage_extension_menu(&mut self, id: &str, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(extension) = self.find_extension(id) else { return };
        let actions = self.manage_extension_actions(&extension);
        let anchor = gpui::Bounds::new(position, gpui::size(px(0.), px(0.)));
        self.show_context_menu_actions(actions, &Default::default(), anchor, vec![json!(extension.identifier().id)], window, cx);
    }

    fn manage_extension_actions(&self, extension: &Extension) -> Vec<MenuAction> {
        let command = |id: &str, title: &str| MenuAction::Command { id: id.into(), title: title.into(), enabled: true, checked: false, alt: None };
        let installed = extension.state == ExtensionState::Installed;
        let gallery = extension.gallery.is_some();
        let has_workspace = self.folder.is_some();
        let mut groups: Vec<Vec<MenuAction>> = Vec::new();
        if installed {
            let disabled = !extension.enablement.is_enabled();
            let mut enable = Vec::new();
            let mut disable = Vec::new();
            if !extension.is_language_pack() {
                if disabled {
                    enable.push(command("extensions.enableGlobally", "Enable"));
                    if has_workspace {
                        enable.push(command("extensions.enableForWorkspace", "Enable (Workspace)"));
                    }
                } else {
                    disable.push(command("extensions.disableGlobally", "Disable"));
                    if has_workspace {
                        disable.push(command("extensions.disableForWorkspace", "Disable (Workspace)"));
                    }
                }
            }
            groups.push(enable);
            groups.push(disable);
            if !extension.is_builtin() {
                let checked = self.is_auto_update_enabled_for(extension);
                groups.push(vec![MenuAction::Command { id: "workbench.extensions.action.toggleAutoUpdateForExtension".into(), title: "Auto Update".into(), enabled: true, checked, alt: None }]);
                groups.push(vec![command("extensions.uninstall", "Uninstall")]);
            }
        } else if extension.state == ExtensionState::Uninstalled && gallery {
            groups.push(vec![command("extensions.install", "Install")]);
        }
        let mut copy = vec![command("workbench.extensions.action.copyExtension", "Copy"), command("workbench.extensions.action.copyExtensionId", "Copy Extension ID")];
        if gallery {
            copy.push(command("workbench.extensions.action.copyLink", "Copy Link"));
        }
        groups.push(copy);
        let manifest = extension.local.as_ref().map(|local| &local.description);
        let mut configure = Vec::new();
        if installed && manifest.is_some_and(|manifest| !manifest["contributes"]["configuration"].is_null()) {
            configure.push(command("workbench.extensions.action.configure", "Settings"));
        }
        if installed && manifest.is_some_and(|manifest| manifest["contributes"]["keybindings"].as_array().is_some_and(|keys| !keys.is_empty())) {
            configure.push(command("workbench.extensions.action.configureKeybindings", "Keyboard Shortcuts"));
        }
        groups.push(configure);
        let mut actions = Vec::new();
        for group in groups.into_iter().filter(|group| !group.is_empty()) {
            if !actions.is_empty() {
                actions.push(MenuAction::Separator);
            }
            actions.extend(group);
        }
        actions
    }

    /// Up, Down, Home, End and Enter in the focused list (listWidget.ts), across the views.
    pub(super) fn move_extensions_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<(usize, usize)> = self.extensions_viewlet.panes.iter().enumerate().filter(|(_, pane)| pane.expanded).flat_map(|(index, pane)| (0..pane.extensions.len()).map(move |row| (index, row))).collect();
        if rows.is_empty() {
            return;
        }
        let current = self.extensions_viewlet.focused.and_then(|focused| rows.iter().position(|row| *row == focused));
        let next = match (current, delta) {
            (None, _) => 0,
            (Some(_), isize::MIN) => 0,
            (Some(_), isize::MAX) => rows.len() - 1,
            (Some(current), delta) => (current as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
        };
        let key = rows[next];
        self.extensions_viewlet.focused = Some(key);
        self.extensions_viewlet.selected = Some(key);
        self.extensions_viewlet.panes[key.0].scroll.scroll_to_item(key.1, ScrollStrategy::Top);
        if let Some(extension) = self.extensions_viewlet.panes[key.0].extensions.get(key.1).cloned() {
            self.open_extension_editor(extension, true, window, cx);
        }
        cx.notify();
    }
}

/// extensionsViewlet.ts `FilterExtensionsAction` submenu: the queries it types into the search box.
fn extensions_filter_actions() -> Vec<MenuAction> {
    let command = |id: String, title: &str| MenuAction::Command { id, title: title.into(), enabled: true, checked: false, alt: None };
    let categories = crate::extensions_model::CATEGORIES.iter().map(|category| command(format!("extensions.actions.searchByCategory.{category}"), category)).collect();
    let sorts = SORTS.iter().map(|(id, title)| command(format!("extensions.sort.{id}"), title)).collect();
    vec![
        command("extensions.filter.featured".into(), "Featured"),
        command("workbench.extensions.action.showPopularExtensions".into(), "Most Popular"),
        command("workbench.extensions.action.showRecommendedExtensions".into(), "Recommended"),
        command("workbench.extensions.action.recentlyPublishedExtensions".into(), "Recently Published"),
        MenuAction::Separator,
        MenuAction::Submenu { title: "Category".into(), actions: categories, checked: false },
        MenuAction::Separator,
        command("workbench.extensions.action.listBuiltInExtensions".into(), "Built-in"),
        command("workbench.extensions.action.extensionUpdates".into(), "Updates"),
        command("workbench.extensions.action.listWorkspaceUnsupportedExtensions".into(), "Workspace Unsupported"),
        command("workbench.extensions.action.showEnabledExtensions".into(), "Enabled"),
        command("workbench.extensions.action.showDisabledExtensions".into(), "Disabled"),
        command("workbench.extensions.action.installedExtensions".into(), "Installed"),
        MenuAction::Separator,
        MenuAction::Submenu { title: "Sort By".into(), actions: sorts, checked: false },
    ]
}

/// The Sort By submenu: `extensions.sort.<id>` and its title.
pub(super) const SORTS: [(&str, &str); 5] = [("installs", "Install Count"), ("rating", "Rating"), ("name", "Name"), ("publishedDate", "Published Date"), ("updateDate", "Updated Date")];

fn extension_local_icon(extension: &Extension) -> Option<std::path::PathBuf> {
    let local = extension.local.as_ref()?;
    let icon = local.description["icon"].as_str()?;
    Some(local.location.join(icon))
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
