//! The extension editor (extensionEditor.ts, media/extensionEditor.css): the header with icon, name, publisher,
//! install count, rating, description and actions; the Details, Features and Changelog tabs. Details and
//! Changelog render the extension's markdown in a webview with the upstream document styles; the Details tab
//! has the Installation, Marketplace, Categories and Resources side panel.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use exthost::gallery::{Gallery, GalleryExtension};
use gpui::{ClickEvent, Context, Div, FontWeight, MouseButton, Window, canvas, div, prelude::*, px};
use super::markdown_document::{Sanitizer, render_markdown_document};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::TextDocument;
use crate::extensions_model::Extension;
use crate::webview::{self, Webview};

/// The scheme of an extension editor's model: `extension:<id>` (ExtensionsInput `resource`).
pub const EXTENSION_EDITOR_SCHEME: &str = "extension";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Details,
    Features,
    Changelog,
}

/// A loaded markdown document: `None` while loading, `Err` with the "No README available." text.
type Markdown = Option<Result<String, ()>>;

pub struct ExtensionEditor {
    pub id: String,
    pub path: PathBuf,
    /// The gallery entry the editor was opened with, kept for an extension that is not installed.
    pub gallery: Option<GalleryExtension>,
    pub tab: Tab,
    readme: Markdown,
    changelog: Markdown,
    /// The webview of the shown markdown tab and the tab it shows.
    webview: Option<(Tab, Webview)>,
    /// The selected feature of the Features tab.
    feature: usize,
}

impl Workbench {
    /// `openExtension`: the extension's editor, reused when it is open; a click in the list opens it as preview.
    pub(super) fn open_extension_editor(&mut self, extension: Extension, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = extension.identifier().id;
        let path = PathBuf::from(format!("{}", id.to_lowercase()));
        if let Some(index) = self.editors.editors.iter().position(|editor| editor.document().is(EXTENSION_EDITOR_SCHEME, &path)) {
            self.editors.activate(index);
        } else {
            let document = TextDocument::from_text(EXTENSION_EDITOR_SCHEME, &path, "", "utf8".into(), None, &self.languages, &self.settings.lock().unwrap());
            self.extension_editors.retain(|editor| editor.path != path);
            self.extension_editors.push(ExtensionEditor { id: id.clone(), path: path.clone(), gallery: extension.gallery.clone(), tab: Tab::Details, readme: None, changelog: None, webview: None, feature: 0 });
            self.editors.open(document, preview);
        }
        self.editors_changed(cx);
        self.load_extension_markdown(&path, Tab::Details, window, cx);
        cx.notify();
    }

    /// `extension.open` for an id not installed and not in a view: the gallery entry first.
    pub(super) fn open_gallery_extension_editor(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let fetch = cx.background_executor().spawn(async move {
            let info = exthost::gallery::ExtensionInfo { id, uuid: None, version: None, pre_release: false };
            Gallery::open_vsx(crate::VERSION).get_extensions(&[info], false)
        });
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(found) = fetch.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let Some(gallery) = found.into_iter().next() else { return };
                let local = workbench.local_extensions();
                let extension = Extension { local: None, gallery: Some(gallery.clone()), state: crate::extensions_model::ExtensionState::Uninstalled, enablement: crate::extension_enablement::EnablementState::EnabledGlobally, running: false, restart_required: false };
                let extension = local.into_iter().find(|local| local.identifier().id.eq_ignore_ascii_case(&gallery.id)).map(|local| Extension { gallery: Some(gallery), ..local }).unwrap_or(extension);
                workbench.open_extension_editor(extension, false, window, cx);
            });
        })
        .detach();
    }

    /// The editor's extension as the workbench now knows it.
    fn editor_extension(&self, editor: &ExtensionEditor) -> Option<Extension> {
        let mut extension = self.find_extension(&editor.id)?;
        if extension.gallery.is_none() {
            extension.gallery = editor.gallery.clone();
        }
        Some(extension)
    }

    /// After a change: editors whose extension was uninstalled keep their gallery entry; the markdown stays.
    pub(super) fn refresh_extension_editors(&mut self) {
        let open: Vec<PathBuf> = self.editors.editors.iter().filter(|editor| editor.document().scheme == EXTENSION_EDITOR_SCHEME).map(|editor| editor.document().path.clone()).collect();
        self.extension_editors.retain(|editor| open.contains(&editor.path));
    }

    /// The README or CHANGELOG: the installed copy's file, else the gallery asset (`ExtensionEditor.extensionReadme`).
    fn load_extension_markdown(&mut self, path: &Path, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.extension_editors.iter().find(|editor| editor.path == path) else { return };
        let loaded = match tab {
            Tab::Details => editor.readme.is_some(),
            Tab::Changelog => editor.changelog.is_some(),
            Tab::Features => true,
        };
        if loaded {
            return;
        }
        let Some(extension) = self.editor_extension(editor) else { return };
        let local = extension.local.as_ref().map(|local| local.location.clone());
        let gallery = extension.gallery.clone();
        let name = if tab == Tab::Details { "readme" } else { "changelog" };
        let fetch = cx.background_executor().spawn(async move {
            if let Some(text) = local.as_deref().and_then(|folder| local_markdown(folder, name)) {
                return Ok(text);
            }
            let gallery_extension = gallery.ok_or(())?;
            let gallery = Gallery::open_vsx(crate::VERSION);
            let text = if name == "readme" { gallery.get_readme(&gallery_extension) } else { gallery.get_changelog(&gallery_extension) };
            text.map_err(|_| ())
        });
        let path = path.to_owned();
        cx.spawn_in(window, async move |workbench, cx| {
            let text = fetch.await;
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(editor) = workbench.extension_editors.iter_mut().find(|editor| editor.path == path) else { return };
                match tab {
                    Tab::Details => editor.readme = Some(text),
                    _ => editor.changelog = Some(text),
                }
                editor.webview = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_extension_tab(&mut self, path: &Path, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.extension_editors.iter_mut().find(|editor| editor.path == path) {
            editor.tab = tab;
            editor.webview = None;
        }
        self.load_extension_markdown(path, tab, window, cx);
        cx.notify();
    }

    /// Runs each render: the active extension editor's markdown has its webview, the others none.
    pub(super) fn update_extension_editor_webviews(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.editors.active().filter(|editor| editor.document().scheme == EXTENSION_EDITOR_SCHEME).map(|editor| editor.document().path.clone());
        let covered = self.quick_inputs.current().is_some() || self.keybindings_editor.open || self.context_menu.is_some();
        let styles = self.extension_markdown_styles();
        let theme_styles = self.webview_theme_styles();
        let events = self.webview_events(window, cx);
        let pre = self.app_root.join("src/vs/workbench/contrib/webview/browser/pre");
        for editor in &mut self.extension_editors {
            let shown = active.as_deref() == Some(editor.path.as_path());
            if !shown {
                editor.webview = None;
                continue;
            }
            let markdown = match editor.tab {
                Tab::Details => &editor.readme,
                Tab::Changelog => &editor.changelog,
                Tab::Features => {
                    editor.webview = None;
                    continue;
                }
            };
            let Some(Ok(text)) = markdown else { continue };
            if let Some((_, webview)) = &editor.webview {
                if covered {
                    webview.hide();
                }
                continue;
            }
            let id = format!("extension-editor-{}", editor.id.to_lowercase());
            let mut webview = Webview::new(window, &id, &webview_origin(&editor.id), "", pre.clone(), Vec::new(), events.clone());
            webview.send("styles", theme_styles.clone());
            webview.send("content", json!({ "contents": render_body(&styles, &render_markdown_document(text, &README_SANITIZER)), "title": if editor.tab == Tab::Details { "Readme" } else { "Changelog" }, "options": { "allowMultipleAPIAcquire": false, "allowScripts": false, "allowForms": false }, "state": Value::Null, "cspSource": "'self' https://*.vscode-cdn.net", "confirmBeforeClose": "keyboardOnly" }));
            editor.webview = Some((editor.tab, webview));
        }
    }

    /// A webview event of an extension editor (`onDidClickLink`): http, https and mailto links open outside.
    pub(super) fn handle_extension_editor_event(&mut self, id: &str, channel: &str, data: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.extension_editors.iter_mut().find(|editor| format!("extension-editor-{}", editor.id.to_lowercase()) == id) else { return };
        let Some((_, webview)) = &mut editor.webview else { return };
        match channel {
            "webview-ready" => webview.did_become_ready(),
            "did-click-link" => {
                let link = data["uri"].as_str().unwrap_or_default();
                if ["http:", "https:", "mailto:"].iter().any(|scheme| link.starts_with(scheme)) {
                    cx.open_url(link);
                }
            }
            "did-focus" => {
                webview.take_keyboard();
                window.blur(cx);
            }
            _ => {}
        }
    }

    /// `ExtensionsInput.getName`: "Extension: <display name>".
    pub(super) fn extension_editor_name(&self, path: &Path) -> String {
        let editor = self.extension_editors.iter().find(|editor| editor.path == path);
        let name = editor.and_then(|editor| self.editor_extension(editor)).map(|extension| extension.display_name().to_owned());
        format!("Extension: {}", name.unwrap_or_else(|| path.to_string_lossy().into_owned()))
    }

    /// The body of an extension editor (extensionEditor.css `.extension-editor`).
    pub(super) fn extension_editor_body(&self, path: &Path, cx: &mut Context<Self>) -> Div {
        let Some(editor) = self.extension_editors.iter().find(|editor| editor.path == path) else { return div().flex_1() };
        let Some(extension) = self.editor_extension(editor) else { return div().flex_1() };
        let manifest = extension.local.as_ref().map(|local| local.description.clone());
        let has_changelog = extension.local.as_ref().is_some_and(|local| local_markdown_path(&local.location, "changelog").is_some()) || extension.gallery.as_ref().is_some_and(|gallery| gallery.changelog.is_some());
        let mut tabs = vec![(Tab::Details, "Details")];
        if manifest.is_some() || extension.gallery.as_ref().is_some_and(|gallery| gallery.manifest.is_some()) {
            tabs.push((Tab::Features, "Features"));
        }
        if has_changelog {
            tabs.push((Tab::Changelog, "Changelog"));
        }
        let content = match editor.tab {
            Tab::Details => self.extension_details(editor, &extension),
            Tab::Changelog => self.extension_markdown_area(editor.webview.as_ref().map(|(_, webview)| webview), &editor.changelog, "No Changelog available."),
            Tab::Features => self.extension_features(editor, manifest.as_ref(), cx),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .mx(px(16.))
            .child(self.extension_editor_header(&extension, cx))
            .child(self.extension_navbar(path, editor.tab, &tabs, cx))
            .child(div().flex_1().min_h_0().flex().child(content))
    }

    fn extension_editor_header(&self, extension: &Extension, cx: &mut Context<Self>) -> Div {
        let description_color = self.foreground_or("foreground");
        let mut subtitle = div().pt(px(6.)).line_height(px(20.)).flex().flex_wrap().items_center();
        let entry = |content: Div, last: bool| {
            content.when(!last, |content| content.border_r_1().border_color(gpui::rgba(0x808080b3)).mr(px(14.)).pr(px(14.)))
        };
        let install_count = extension.install_count().filter(|count| *count > 0.);
        let rating = extension.rating().filter(|_| extension.gallery.is_some());
        subtitle = subtitle.child(entry(div().child(extension.publisher_display_name().to_owned()), install_count.is_none() && rating.is_none()));
        if let Some(count) = install_count {
            subtitle = subtitle.child(entry(div().flex().items_center().child(self.icon("cloud-download", 16., 16., description_color)).child(div().ml(px(6.)).child(group_thousands(count as u64))), rating.is_none()));
        }
        if let Some(rating) = rating {
            let rating = (rating * 2.).round() / 2.;
            let star = self.color("extensionIcon.starForeground");
            let mut stars = div().flex().items_center();
            for index in 1..=5 {
                let icon = match () {
                    _ if rating >= index as f64 => "star-full",
                    _ if rating >= index as f64 - 0.5 => "star-half",
                    _ => "star-empty",
                };
                stars = stars.child(self.icon(icon, 16., 16., star).when(icon == "star-empty", |star| star.opacity(0.75)));
            }
            if let Some(count) = extension.rating_count().filter(|count| *count > 0.) {
                stars = stars.child(div().pl(px(1.)).child(format!(" ({count})")));
            }
            subtitle = subtitle.child(stars);
        }
        let mut actions = div().mt(px(10.)).flex().flex_wrap().items_center();
        for button in self.extension_buttons(extension, true) {
            actions = actions.child(self.extension_button(button, 22., cx).mr(px(6.)));
        }
        // `ToggleAutoUpdateForExtensionAction.update`: not for built-ins, nor for a disabled one while auto update is on.
        if !extension.is_builtin() && !(self.auto_update_on() && !extension.enablement.is_enabled()) {
            let checked = self.is_auto_update_enabled_for(extension);
            let id = extension.identifier().id;
            actions = actions.child(
                div()
                    .id("extension-auto-update")
                    .cursor_pointer()
                    .mr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(18.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(self.color("checkbox.border"))
                            .bg(self.color("checkbox.background"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(checked, |checkbox| checkbox.child(self.icon("check", 16., 16., self.foreground_or("checkbox.foreground")))),
                    )
                    .child("Auto Update")
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| {
                        drop(workbench.execute_command("workbench.extensions.action.toggleAutoUpdateForExtension", vec![json!(id)], window, cx));
                    })),
            );
        }
        // `ExtensionEditorManageExtensionAction` shows for every state.
        {
            let id = extension.identifier().id;
            actions = actions.child(self.icon("gear", 22., 16., self.foreground_or("icon.foreground")).id("extension-editor-manage").rounded(px(4.)).cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    workbench.show_manage_extension_menu(&id, event.position, window, cx);
                }),
            ));
        }
        let mut title = div().flex().items_center().child(div().text_size(px(26.)).line_height(px(30.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().child(extension.display_name().to_owned()));
        if extension.is_builtin() {
            title = title.child(div().ml(px(10.)).text_size(px(10.)).italic().child("Built-In"));
        }
        // extensionEditor.ts `VersionWidget`: a `code.version` "pre-release" only for an installed pre-release.
        if extension.pre_release() {
            title = title.child(div().ml(px(10.)).whitespace_nowrap().font_family("Menlo").text_size(px(13.)).child("pre-release"));
        }
        div()
            .flex_none()
            .flex()
            .pt(px(20.))
            .pb(px(12.))
            .pl(px(20.))
            .overflow_hidden()
            .text_size(px(14.))
            .child(self.extension_icon(extension, 128.))
            .child(div().pl(px(12.)).overflow_hidden().child(title).child(subtitle).child(div().mt(px(10.)).child(extension.description().to_owned())).child(actions))
    }

    /// The navbar: 36px, the tabs in panel title colors, the checked one underlined.
    fn extension_navbar(&self, path: &Path, active: Tab, tabs: &[(Tab, &'static str)], cx: &mut Context<Self>) -> Div {
        let mut navbar = div().h(px(37.)).flex_none().flex().pl(px(20.)).border_b_1().border_color(self.color("panelSection.border"));
        for (tab, label) in tabs.iter().copied() {
            let checked = tab == active;
            let path = path.to_owned();
            let foreground = if checked { self.foreground_or("panelTitle.activeForeground") } else { self.foreground_or("panelTitle.inactiveForeground") };
            let hover = self.foreground_or("panelTitle.activeForeground");
            navbar = navbar.child(
                div()
                    .id(label)
                    .cursor_pointer()
                    .h_full()
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(foreground)
                    .hover(move |tab| tab.text_color(hover))
                    .when(checked, |tab| tab.border_b_1().border_color(self.color("panelTitle.activeBorder")))
                    .child(label.to_uppercase())
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, window, cx| workbench.switch_extension_tab(&path, tab, window, cx))),
            );
        }
        navbar
    }

    /// Details: the README (75% at most, centered) and the 25% side panel.
    fn extension_details(&self, editor: &ExtensionEditor, extension: &Extension) -> Div {
        let webview = editor.webview.as_ref().filter(|(tab, _)| *tab == Tab::Details).map(|(_, webview)| webview);
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .child(div().flex_1().min_w_0().max_w(gpui::relative(0.75)).mx_auto().h_full().flex().child(self.extension_markdown_area(webview, &editor.readme, "No README available.")))
            .child(div().w(gpui::relative(0.25)).min_w(px(175.)).h_full().child(self.additional_details(extension)))
    }

    /// A markdown tab: the webview placed over an empty box, or the no-content text.
    fn extension_markdown_area(&self, webview: Option<&Webview>, markdown: &Markdown, no_content: &str) -> Div {
        if let Some(Err(())) = markdown {
            return div().flex_1().child(div().ml(px(20.)).mt(px(13.)).child(no_content.to_owned()));
        }
        let view = webview.map(Webview::view);
        div().flex_1().min_h_0().h_full().child(
            canvas(
                move |bounds, window, _| {
                    if let Some(view) = &view {
                        webview::place(view, window, bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
    }

    /// `AdditionalDetailsWidget`: Installation, Marketplace, Categories and Resources.
    fn additional_details(&self, extension: &Extension) -> Div {
        let mut sections: Vec<Div> = Vec::new();
        let rows = |title: &str, entries: Vec<(&str, String, bool)>| {
            let mut info = div().flex().flex_col();
            for (index, (name, value, code)) in entries.into_iter().enumerate() {
                let value = if code { div().font_family("Menlo").child(value) } else { div().child(value) };
                info = info.child(
                    div().text_size(px(11.7)).flex().gap(px(6.)).px(px(4.)).py(px(2.)).when(index % 2 == 0, |row| row.bg(gpui::rgba(0x8282820a))).child(div().w(gpui::relative(0.4)).overflow_hidden().text_ellipsis().child(name.to_owned())).child(div().flex_1().min_w_0().child(value)),
                );
            }
            div().child(div().text_size(px(15.6)).pb(px(12.)).child(title.to_owned())).child(info)
        };
        if let Some(local) = &extension.local {
            let mut entries = vec![("Identifier", local.id.clone(), true)];
            if !extension.is_builtin() {
                entries.push(("Version", extension.version().to_owned(), true));
            }
            if let Some(installed) = extension.installed_timestamp() {
                entries.push(("Last Updated", from_now(installed as f64), false));
            }
            let source = local.description["__metadata"]["source"].as_str().unwrap_or_default();
            if !extension.is_builtin() && source != "gallery" {
                entries.push(("Source", if source == "vsix" { "VSIX".into() } else { "Local".into() }, false));
            }
            if let Some(size) = local.description["__metadata"]["size"].as_u64() {
                entries.push(("Size", format_size(size), false));
            }
            sections.push(rows("Installation", entries));
        }
        if let Some(gallery) = &extension.gallery {
            let mut entries = Vec::new();
            if extension.local.is_none() {
                entries.push(("Identifier", gallery.id.clone(), true));
                entries.push(("Version", gallery.version.clone(), true));
            }
            if let Some(published) = super::extensions_view::iso_millis(&gallery.release_date) {
                entries.push(("Published", from_now(published), false));
            }
            if let Some(updated) = super::extensions_view::iso_millis(&gallery.last_updated) {
                entries.push(("Last Released", from_now(updated), false));
            }
            sections.push(rows("Marketplace", entries));
        }
        let categories = extension.categories();
        if !categories.is_empty() {
            let mut list = div().flex().flex_wrap();
            for category in categories {
                list = list.child(div().border_1().border_color(gpui::rgba(0x88888873)).px(px(4.)).py(px(2.)).rounded(px(2.)).text_size(px(11.7)).mr(px(6.)).mb(px(3.)).child(category));
            }
            sections.push(div().child(div().text_size(px(15.6)).pb(px(12.)).child("Categories")).child(list));
        }
        let mut resources: Vec<(&str, String)> = Vec::new();
        if let Some(gallery) = &extension.gallery {
            if let Some(repository) = &gallery.repository {
                resources.push(("Repository", repository.clone()));
            }
            if let Some(license) = &gallery.license {
                resources.push(("License", license.uri.clone()));
            }
            resources.push(("Marketplace", gallery.detail_link.clone()));
        }
        if !resources.is_empty() {
            let link = self.color("textLink.foreground");
            let mut list = div().flex().flex_col();
            for (label, uri) in resources {
                let icon = if label == "Repository" { "repo" } else { "link-external" };
                list = list.child(
                    div()
                        .id(gpui::SharedString::from(format!("resource-{label}")))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .px(px(4.))
                        .py(px(2.))
                        .child(self.icon(icon, 16., 16., self.foreground_or("foreground")))
                        .child(div().pl(px(4.)).text_color(link).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.to_owned()))
                        .on_click(move |_, _, cx| cx.open_url(&uri)),
                );
            }
            sections.push(div().child(div().text_size(px(15.6)).pb(px(12.)).child("Resources")).child(list));
        }
        let count = sections.len();
        let mut content = div().id("additional-details").overflow_y_scroll().h_full().p(px(12.)).flex().flex_col();
        for (index, section) in sections.into_iter().enumerate() {
            content = content.child(section.when(index > 0, |section| section.pt(px(16.))).pb(px(16.)).when(index + 1 < count, |section| section.border_b_1().border_color(gpui::rgba(0x80808038))));
        }
        div().h_full().child(content)
    }

    /// Features (extensionFeaturesTab.ts): the contributed features on the left, the selected one's entries on the right.
    // shortcut: each feature shows its entries as a list of ids or titles, not the upstream renderers' tables.
    fn extension_features(&self, editor: &ExtensionEditor, manifest: Option<&Value>, cx: &mut Context<Self>) -> Div {
        let Some(manifest) = manifest else { return div().flex_1().child(div().ml(px(20.)).mt(px(13.)).child("No features contributed.")) };
        let features: Vec<(&str, &str)> = FEATURE_LABELS.iter().copied().filter(|(id, _)| crate::extensions_model::contributes(manifest, id)).collect();
        if features.is_empty() {
            return div().flex_1().child(div().ml(px(20.)).mt(px(13.)).child("No features contributed."));
        }
        let selected = editor.feature.min(features.len() - 1);
        let mut list = div().w(px(200.)).flex_none().flex().flex_col().pt(px(8.));
        for (index, (_, label)) in features.iter().enumerate() {
            let path = editor.path.clone();
            list = list.child(
                div()
                    .id(("feature", index))
                    .cursor_pointer()
                    .h(px(22.))
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .when(index == selected, |row| row.bg(self.color("list.inactiveSelectionBackground")))
                    .hover(|row| row.bg(self.color("list.hoverBackground")))
                    .child(label.to_string())
                    .on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                        if let Some(editor) = workbench.extension_editors.iter_mut().find(|editor| editor.path == path) {
                            editor.feature = index;
                        }
                        cx.notify();
                    })),
            );
        }
        let (id, label) = features[selected];
        let entries = feature_entries(manifest, id);
        let mut detail = div().id("feature-detail").flex_1().overflow_y_scroll().p(px(12.)).flex().flex_col().child(div().text_size(px(15.6)).pb(px(12.)).child(label.to_owned()));
        for entry in entries {
            detail = detail.child(div().py(px(2.)).child(entry));
        }
        div().flex_1().min_w_0().flex().child(list).child(detail)
    }

    /// The `--vscode-*` theme data the webview host page applies, the panels' own.
    fn webview_theme_styles(&self) -> Value {
        let label = self.color_themes.iter().find(|entry| entry.settings_id == self.theme.settings_id).map_or(self.theme.settings_id.clone(), |entry| entry.label.clone());
        webview::theme_data(&self.theme, &label, &self.setting("editor"), self.setting("accessibility.underlineLinks") == true, self.is_motion_reduced())
    }

    /// markdownDocumentRenderer.ts `DEFAULT_MARKDOWN_STYLES`, read from the shipped source.
    fn extension_markdown_styles(&self) -> String {
        static STYLES: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        STYLES
            .get_or_init(|| {
                let source = std::fs::read_to_string(self.app_root.join("src/vs/workbench/contrib/markdown/browser/markdownDocumentRenderer.ts")).unwrap_or_default();
                let start = source.find("DEFAULT_MARKDOWN_STYLES = `").map(|start| start + "DEFAULT_MARKDOWN_STYLES = `".len());
                start.and_then(|start| source[start..].find('`').map(|end| source[start..start + end].to_owned())).unwrap_or_default()
            })
            .clone()
    }
}

/// extensionEditor.ts `renderMarkdown`: links to the web and mail only.
// shortcut: built-in extensions' `command:` links are dropped too; the editor does not run command links.
const README_SANITIZER: Sanitizer = Sanitizer { link_protocols: Some(&["http", "https", "mailto"]), media_protocols: &["http", "https"], allow_relative_media: false };

/// The feature ids of the Features tab with their labels (extensionFeaturesTab.ts and the registered renderers).
const FEATURE_LABELS: [(&str, &str); 12] = [
    ("commands", "Commands"),
    ("configuration", "Settings"),
    ("keybindings", "Keyboard Shortcuts"),
    ("languages", "Programming Languages"),
    ("grammars", "Syntax Highlighting"),
    ("themes", "Color Themes"),
    ("iconThemes", "File Icon Themes"),
    ("productIconThemes", "Product Icon Themes"),
    ("jsonValidation", "JSON Validation"),
    ("debuggers", "Debuggers"),
    ("customEditors", "Custom Editors"),
    ("activationEvents", "Activation Events"),
];

fn feature_entries(manifest: &Value, feature: &str) -> Vec<String> {
    let value = if feature == "activationEvents" { &manifest["activationEvents"] } else { &manifest["contributes"][feature] };
    let describe = |item: &Value| -> String {
        if let Some(text) = item.as_str() {
            return text.to_owned();
        }
        for key in ["command", "id", "label", "language", "key", "fileMatch", "type", "viewType", "title"] {
            match &item[key] {
                Value::String(text) => return text.clone(),
                Value::Array(items) if !items.is_empty() => return items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                _ => {}
            }
        }
        item.to_string()
    };
    match value {
        Value::Array(items) => items.iter().map(describe).collect(),
        // `contributes.configuration`: the setting keys of each node.
        Value::Object(node) if feature == "configuration" => node.get("properties").and_then(Value::as_object).map(|properties| properties.keys().cloned().collect()).unwrap_or_default(),
        Value::Object(fields) => fields.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The extension's README or CHANGELOG file, matched case-insensitively as the scanner does (`readme.md`, `README`).
fn local_markdown_path(folder: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(folder).ok()?;
    entries.flatten().map(|entry| entry.path()).find(|path| path.file_stem().is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(name)) && path.is_file())
}

fn local_markdown(folder: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(local_markdown_path(folder, name)?).ok()
}

/// extensionEditor.ts `renderBody`: the document styles under a CSP without scripts.
fn render_body(styles: &str, body: &str) -> String {
    static NONCE: LazyLock<String> = LazyLock::new(|| format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos())));
    let nonce = &*NONCE;
    format!(
        r#"<!DOCTYPE html>
<html>
	<head>
		<meta http-equiv="Content-type" content="text/html;charset=UTF-8">
		<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src https: data:; media-src https:; script-src 'none'; style-src 'nonce-{nonce}';">
		<style nonce="{nonce}">
			{styles}
			body {{
				padding-bottom: 75px;
			}}
		</style>
	</head>
	<body>
		{body}
	</body>
</html>"#
    )
}

/// A stable webview origin per extension editor.
fn webview_origin(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("extension-editor/{}", id.to_lowercase()).as_bytes());
    let hex: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// `installCount.toLocaleString()` in en-US: 9,442,581.
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `ByteSize.formatSize`.
fn format_size(size: u64) -> String {
    const KB: f64 = 1024.;
    let size = size as f64;
    match () {
        _ if size < KB => format!("{size}B"),
        _ if size < KB * KB => format!("{:.2}KB", size / KB),
        _ if size < KB * KB * KB => format!("{:.2}MB", size / KB / KB),
        _ => format!("{:.2}GB", size / KB / KB / KB),
    }
}

/// date.ts `fromNow(date, true, true, true)`.
pub fn from_now(date_millis: f64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0., |elapsed| elapsed.as_millis() as f64);
    from_now_at(date_millis, now)
}

fn from_now_at(date_millis: f64, now: f64) -> String {
    let seconds = ((now - date_millis) / 1000.).round();
    // A date in the future reads "in 5 days", without "ago".
    let future = seconds < -30.;
    let seconds = seconds.abs();
    let (minute, hour, day) = (60., 3600., 86400.);
    let (week, month, year) = (day * 7., day * 30., day * 365.);
    let (value, unit) = match () {
        _ if seconds < minute => (seconds, "second"),
        _ if seconds < hour => ((seconds / minute).round(), "minute"),
        _ if seconds < day => ((seconds / hour).round(), "hour"),
        _ if seconds < week => ((seconds / day).round(), "day"),
        _ if seconds < month => ((seconds / week).round(), "week"),
        _ if seconds < year => ((seconds / month).round(), "month"),
        _ => ((seconds / year).round(), "year"),
    };
    let plural = if value == 1. { "" } else { "s" };
    match future {
        true => format!("in {value} {unit}{plural}"),
        false => format!("{value} {unit}{plural} ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts_sizes_and_dates() {
        assert_eq!(group_thousands(9442581), "9,442,581");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(2048), "2.00KB");
        let day = 86_400_000.;
        let now = 1_000_000_000_000.;
        assert_eq!(from_now_at(now - 5. * day, now), "5 days ago");
        assert_eq!(from_now_at(now - 1. * day, now), "1 day ago");
        assert_eq!(from_now_at(now - 7. * 365. * day, now), "7 years ago");
        assert_eq!(from_now_at(now - 10_000., now), "10 seconds ago");
    }

    #[test]
    fn lists_feature_entries() {
        let manifest = json!({ "contributes": { "commands": [{ "command": "a.b", "title": "B" }], "configuration": { "properties": { "x.y": {} } } }, "activationEvents": ["onLanguage:yaml"] });
        assert_eq!(feature_entries(&manifest, "commands"), ["a.b"]);
        assert_eq!(feature_entries(&manifest, "configuration"), ["x.y"]);
        assert_eq!(feature_entries(&manifest, "activationEvents"), ["onLanguage:yaml"]);
    }
}
